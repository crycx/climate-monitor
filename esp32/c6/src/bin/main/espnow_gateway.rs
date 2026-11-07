//! ESP-NOW gateway: receives readings from sensor nodes
//! (`esp32/xiao-c6-sensor`, `esp32/c3` in ESP-NOW mode) and forwards them.
//!
//! Two tasks:
//! - [`rx_task`] authenticates packets, drops duplicates, answers discovery
//!   broadcasts with a signed ack and queues new readings.
//! - [`forward_task`] publishes queued readings to Home Assistant over MQTT
//!   (with discovery), and POSTs them to the backend for sensors that have a
//!   backend API key configured on the gateway (sensor MAC -> API key).
//!
//! The gateway is enabled only when the firmware is built with `ESPNOW_KEY`
//! (same value as on the sensors). It must stay awake to receive, so deep
//! sleep should be disabled on the gateway.

use alloc::{format, string::String, vec::Vec};
use core::cell::RefCell;
use core::sync::atomic::{AtomicU32, Ordering};

use embassy_net::{Stack, tcp::{TcpSocket, client::TcpClientState}, IpEndpoint};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, mutex::Mutex};
use embassy_time::{Duration, Instant};
use esp_radio::esp_now::{EspNow, EspNowWifiInterface, PeerInfo};

use crate::app_config::AppConfig;
use crate::espnow_packet::{self, Ack, DecodeError, Packet, Reading};
use crate::log_runtime_line;

const QUEUE_DEPTH: usize = 8;
const MAX_TRACKED_SENSORS: usize = 16;
const MIN_KEY_LEN: usize = 8;

#[derive(Clone, Copy)]
pub struct RemoteReading {
    pub mac: [u8; 6],
    pub reading: Reading,
    pub rssi: i32,
}

static READINGS: Channel<CriticalSectionRawMutex, RemoteReading, QUEUE_DEPTH> = Channel::new();

// ── Latest reading per sensor, for the OLED ─────────────────────────────────

const DISPLAY_SLOTS: usize = 8;
/// Used when a sensor does not report its interval.
const DEFAULT_STALE_SECS: u64 = 900;

#[derive(Clone, Copy)]
struct DisplaySlot {
    item: RemoteReading,
    received_at: Instant,
}

static DISPLAY: critical_section::Mutex<RefCell<[Option<DisplaySlot>; DISPLAY_SLOTS]>> =
    critical_section::Mutex::new(RefCell::new([None; DISPLAY_SLOTS]));
/// Bumped on every new reading so the main loop can jump to that sensor's page.
static DISPLAY_GENERATION: AtomicU32 = AtomicU32::new(0);

fn stale_after_secs(r: &Reading) -> u64 {
    if r.interval_s == 0 {
        DEFAULT_STALE_SECS
    } else {
        r.interval_s as u64 * 3 + 60
    }
}

fn remember_for_display(item: &RemoteReading) {
    let now = Instant::now();
    critical_section::with(|cs| {
        let mut slots = DISPLAY.borrow_ref_mut(cs);
        // Same sensor, else a free slot, else the least recently heard one.
        let index = slots
            .iter()
            .position(|s| s.is_some_and(|s| s.item.mac == item.mac))
            .or_else(|| slots.iter().position(Option::is_none))
            .unwrap_or_else(|| {
                slots
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, s)| s.map(|s| s.received_at))
                    .map(|(i, _)| i)
                    .unwrap_or(0)
            });
        slots[index] = Some(DisplaySlot {
            item: *item,
            received_at: now,
        });
    });
    DISPLAY_GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// Every sensor heard since boot: (MAC, name, seconds since last reading).
pub fn seen_sensors() -> Vec<([u8; 6], String, u64)> {
    let now = Instant::now();
    critical_section::with(|cs| {
        DISPLAY
            .borrow_ref(cs)
            .iter()
            .flatten()
            .map(|s| {
                (
                    s.item.mac,
                    String::from(s.item.reading.name().trim()),
                    now.duration_since(s.received_at).as_secs(),
                )
            })
            .collect()
    })
}

pub fn display_generation() -> u32 {
    DISPLAY_GENERATION.load(Ordering::Relaxed)
}

/// Text for one OLED page.
pub struct RemotePage {
    pub name: String,
    pub temp: Option<f32>,
    pub hum: Option<f32>,
    pub battery_pct: Option<u8>,
    pub footer: String,
}

fn format_age(secs: u64) -> String {
    if secs < 60 {
        format!("{}s ago", secs)
    } else if secs < 3600 {
        format!("{}min ago", secs / 60)
    } else {
        format!("{}h ago", secs / 3600)
    }
}

/// Pages for sensors heard recently (stable slot order), plus the index of
/// the most recently heard one.
pub fn display_pages() -> (Vec<RemotePage>, Option<usize>) {
    let now = Instant::now();
    let fresh: Vec<DisplaySlot> = critical_section::with(|cs| {
        DISPLAY
            .borrow_ref(cs)
            .iter()
            .flatten()
            .filter(|s| now.duration_since(s.received_at).as_secs() <= stale_after_secs(&s.item.reading))
            .copied()
            .collect()
    });

    let newest = fresh
        .iter()
        .enumerate()
        .max_by_key(|(_, s)| s.received_at)
        .map(|(i, _)| i);

    let pages = fresh
        .iter()
        .map(|s| {
            let r = &s.item.reading;
            let name = r.name().trim();
            let ok = r.sensor_ok();
            RemotePage {
                name: if name.is_empty() {
                    format!("Sensor {}", &mac_hex(&s.item.mac)[6..])
                } else {
                    name.into()
                },
                temp: ok.then(|| r.temperature_c()),
                hum: ok.then(|| r.humidity_pct()),
                battery_pct: (r.battery_mv > 0).then(|| espnow_packet::lipo_percent(r.battery_mv)),
                footer: format!(
                    "ESP-NOW  {}  {}dBm",
                    format_age(now.duration_since(s.received_at).as_secs()),
                    s.item.rssi
                ),
            }
        })
        .collect();
    (pages, newest)
}

/// Shared HMAC key; ESP-NOW is disabled when it is missing or too short.
pub fn key() -> Option<&'static [u8]> {
    option_env!("ESPNOW_KEY")
        .filter(|k| k.len() >= MIN_KEY_LEN)
        .map(str::as_bytes)
}

pub fn mac_hex(mac: &[u8; 6]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    )
}

/// Last (boot_id, seq) per sensor so retransmits and replays are dropped.
struct SeenTable {
    entries: [Option<([u8; 6], u32, u32)>; MAX_TRACKED_SENSORS],
    next_slot: usize,
}

impl SeenTable {
    const fn new() -> Self {
        Self {
            entries: [None; MAX_TRACKED_SENSORS],
            next_slot: 0,
        }
    }

    /// Returns true if this (boot_id, seq) is new for `mac`, and records it.
    fn accept(&mut self, mac: &[u8; 6], boot_id: u32, seq: u32) -> bool {
        for entry in self.entries.iter_mut().flatten() {
            if &entry.0 == mac {
                let is_new = entry.1 != boot_id || seq > entry.2;
                if is_new {
                    entry.1 = boot_id;
                    entry.2 = seq;
                }
                return is_new;
            }
        }
        self.entries[self.next_slot] = Some((*mac, boot_id, seq));
        self.next_slot = (self.next_slot + 1) % MAX_TRACKED_SENSORS;
        true
    }
}

async fn send_ack(esp_now: &mut EspNow<'static>, key: &[u8], own_mac: &[u8; 6], dst: &[u8; 6], r: &Reading) {
    let ack = Ack {
        boot_id: r.boot_id,
        seq: r.seq,
        // 0 = "the channel you heard this on"; the sensor tracks that itself.
        channel: 0,
    };
    let mut buf = [0u8; espnow_packet::MAX_PACKET_LEN];
    let len = espnow_packet::encode_ack(key, own_mac, &ack, &mut buf);

    // Unicast needs a peer entry. Remove it afterwards so the 20-entry peer
    // table never fills up with sensors that only talk to us once.
    let added = if esp_now.peer_exists(dst) {
        false
    } else {
        match esp_now.add_peer(PeerInfo {
            interface: EspNowWifiInterface::Station,
            peer_address: *dst,
            lmk: None,
            channel: None,
            encrypt: false,
        }) {
            Ok(()) => true,
            Err(e) => {
                log_runtime_line(format!("ESP-NOW: add_peer for ack failed: {:?}", e).as_str());
                return;
            }
        }
    };

    if let Err(e) = esp_now.send_async(dst, &buf[..len]).await {
        log_runtime_line(format!("ESP-NOW: ack to {} failed: {:?}", mac_hex(dst), e).as_str());
    }

    if added {
        let _ = esp_now.remove_peer(dst);
    }
}

#[embassy_executor::task]
pub async fn rx_task(mut esp_now: EspNow<'static>, own_mac: [u8; 6]) {
    let Some(key) = key() else {
        return;
    };
    log_runtime_line(
        format!("ESP-NOW: gateway listening, MAC {}", mac_hex(&own_mac)).as_str(),
    );

    let mut seen = SeenTable::new();
    loop {
        let rx = esp_now.receive_async().await;
        let src = rx.info.src_address;
        match espnow_packet::decode(key, &src, rx.data()) {
            Ok(Packet::Reading { reading, wants_ack }) => {
                if wants_ack {
                    send_ack(&mut esp_now, key, &own_mac, &src, &reading).await;
                }
                if !seen.accept(&src, reading.boot_id, reading.seq) {
                    continue;
                }
                let item = RemoteReading {
                    mac: src,
                    reading,
                    // esp-radio 0.18 copies the 8-bit RSSI field without
                    // sign extension on the C6 (-30 dBm arrives as 226).
                    rssi: rx.info.rx_control.rssi as i8 as i32,
                };
                remember_for_display(&item);
                if READINGS.try_send(item).is_err() {
                    log_runtime_line("ESP-NOW: queue full, dropping reading");
                }
            }
            Ok(Packet::Ack(_)) => {}
            Err(DecodeError::BadTag) => {
                log_runtime_line(
                    format!(
                        "ESP-NOW: bad HMAC from {} (ESPNOW_KEY mismatch?)",
                        mac_hex(&src)
                    )
                    .as_str(),
                );
            }
            // Foreign ESP-NOW traffic (other vendors/projects): ignore silently.
            Err(_) => {}
        }
    }
}

struct MqttSettings {
    host: String,
    port: u16,
    username: String,
    password: String,
    base_topic: String,
}

async fn mqtt_settings(config: &'static Mutex<CriticalSectionRawMutex, AppConfig>) -> Option<MqttSettings> {
    let cfg = config.lock().await;
    let enabled = match crate::ACTIVE_WIFI_SLOT.load(Ordering::Relaxed) {
        2 => cfg.mqtt_enabled_2,
        3 => cfg.mqtt_enabled_3,
        4 => cfg.mqtt_enabled_4,
        5 => cfg.mqtt_enabled_5,
        _ => cfg.mqtt_enabled_1,
    };
    let host = cfg.mqtt_host.trim();
    if !enabled || host.is_empty() || host.ends_with(".local") {
        return None;
    }
    let base_topic = cfg.mqtt_base_topic.trim();
    Some(MqttSettings {
        host: host.into(),
        port: crate::parse_mqtt_port(cfg.mqtt_port.as_str()),
        username: cfg.mqtt_username.clone(),
        password: cfg.mqtt_password.clone(),
        base_topic: if base_topic.is_empty() {
            "climate_monitor".into()
        } else {
            base_topic.into()
        },
    })
}

/// Sensors whose discovery config was already sent since boot (MAC + name hash).
struct AnnouncedTable {
    entries: [Option<([u8; 6], u32)>; MAX_TRACKED_SENSORS],
    next_slot: usize,
}

impl AnnouncedTable {
    const fn new() -> Self {
        Self {
            entries: [None; MAX_TRACKED_SENSORS],
            next_slot: 0,
        }
    }

    fn contains(&self, mac: &[u8; 6], name_hash: u32) -> bool {
        self.entries
            .iter()
            .flatten()
            .any(|(m, h)| m == mac && *h == name_hash)
    }

    fn insert(&mut self, mac: &[u8; 6], name_hash: u32) {
        for entry in self.entries.iter_mut().flatten() {
            if &entry.0 == mac {
                entry.1 = name_hash;
                return;
            }
        }
        self.entries[self.next_slot] = Some((*mac, name_hash));
        self.next_slot = (self.next_slot + 1) % MAX_TRACKED_SENSORS;
    }
}

fn fnv1a(s: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for b in s.bytes() {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// Backend URL and this sensor's API key, if the sensor is registered on the gateway.
async fn backend_target(
    config: &'static Mutex<CriticalSectionRawMutex, AppConfig>,
    mac: &str,
) -> Option<(String, String)> {
    let cfg = config.lock().await;
    let api_url = cfg.api_url.trim();
    let api_key = cfg.espnow_backend_api_key(mac)?;
    (!api_url.is_empty()).then(|| (String::from(api_url), String::from(api_key)))
}

#[embassy_executor::task]
pub async fn forward_task(
    stack: &'static Stack<'static>,
    tcp_state: &'static TcpClientState<2, 4096, 4096>,
    config: &'static Mutex<CriticalSectionRawMutex, AppConfig>,
) {
    let mut announced = AnnouncedTable::new();
    loop {
        let item = READINGS.receive().await;
        let r = &item.reading;
        let mac = mac_hex(&item.mac);
        if r.sensor_ok() {
            log_runtime_line(
                format!(
                    "ESP-NOW {} '{}': {:.1} C {:.1} % bat {} mV rssi {} seq {}",
                    mac,
                    r.name(),
                    r.temperature_c(),
                    r.humidity_pct(),
                    r.battery_mv,
                    item.rssi,
                    r.seq
                )
                .as_str(),
            );
        } else {
            log_runtime_line(
                format!(
                    "ESP-NOW {} '{}': sensor read failed, bat {} mV rssi {}",
                    mac,
                    r.name(),
                    r.battery_mv,
                    item.rssi
                )
                .as_str(),
            );
        }

        if stack.config_v4().is_none() {
            log_runtime_line("ESP-NOW: network down, reading not forwarded");
            continue;
        }
        if let Some(settings) = mqtt_settings(config).await {
            let name_hash = fnv1a(r.name());
            let needs_discovery = !announced.contains(&item.mac, name_hash);
            if publish(*stack, &settings, &item, needs_discovery).await && needs_discovery {
                announced.insert(&item.mac, name_hash);
            }
        }

        if r.sensor_ok() {
            if let Some((api_url, api_key)) = backend_target(config, mac.as_str()).await {
                // One attempt: retries would hold up the queue; the next
                // reading follows within one sensor interval anyway.
                let ok = crate::post_readings(
                    stack,
                    tcp_state,
                    api_url.as_str(),
                    api_key.as_str(),
                    r.name(),
                    r.temperature_c(),
                    r.humidity_pct(),
                )
                .await;
                log_runtime_line(
                    format!("ESP-NOW {} -> backend: {}", mac, if ok { "ok" } else { "failed" }).as_str(),
                );
            }
        }
    }
}

async fn with_io_timeout<F: core::future::Future>(fut: F) -> Option<F::Output> {
    embassy_time::with_timeout(Duration::from_secs(crate::MQTT_IO_TIMEOUT_SECS), fut)
        .await
        .ok()
}

struct EntitySpec {
    key: &'static str,
    name: &'static str,
    unit: &'static str,
    device_class: &'static str,
    diagnostic: bool,
}

const ENTITIES: [EntitySpec; 5] = [
    EntitySpec { key: "temperature", name: "Temperature", unit: "°C", device_class: "temperature", diagnostic: false },
    EntitySpec { key: "humidity", name: "Humidity", unit: "%", device_class: "humidity", diagnostic: false },
    EntitySpec { key: "battery", name: "Battery", unit: "%", device_class: "battery", diagnostic: true },
    EntitySpec { key: "battery_voltage", name: "Battery voltage", unit: "V", device_class: "voltage", diagnostic: true },
    EntitySpec { key: "rssi", name: "Signal strength", unit: "dBm", device_class: "signal_strength", diagnostic: true },
];

/// Publishes one reading; returns true if the broker accepted the session.
async fn publish(stack: Stack<'static>, s: &MqttSettings, item: &RemoteReading, discovery: bool) -> bool {
    let Some(ip) = crate::resolve_host_to_ip(stack, s.host.as_str()).await else {
        log_runtime_line("ESP-NOW MQTT: DNS/host resolution failed");
        return false;
    };

    let device_id = format!("espnow_{}", mac_hex(&item.mac));
    let client_id = format!("{}-{}", s.base_topic, device_id);

    let mut rx_buf = [0u8; 512];
    let mut tx_buf = [0u8; 1024];
    let mut socket = TcpSocket::new(stack, &mut rx_buf, &mut tx_buf);
    let connected = embassy_time::with_timeout(
        Duration::from_secs(crate::MQTT_CONNECT_TIMEOUT_SECS),
        socket.connect(IpEndpoint::new(ip, s.port)),
    )
    .await;
    if !matches!(connected, Ok(Ok(()))) {
        log_runtime_line("ESP-NOW MQTT: connect timeout/failed");
        socket.abort();
        return false;
    }
    let session = with_io_timeout(crate::mqtt_connect(
        &mut socket,
        client_id.as_str(),
        s.username.as_str(),
        s.password.as_str(),
    ))
    .await;
    if !matches!(session, Some(Ok(()))) {
        log_runtime_line("ESP-NOW MQTT: CONNECT timeout/failed");
        socket.abort();
        return false;
    }

    let topic = |key: &str| format!("{}/{}/{}", s.base_topic, device_id, key);
    let r = &item.reading;

    if discovery {
        let name = r.name().trim();
        let device_name = if name.is_empty() {
            format!("Climate Sensor {}", &device_id[device_id.len() - 6..])
        } else {
            format!("Climate Sensor ({})", name)
        };
        let device_block = format!(
            r#"{{"identifiers":["{}:{}"],"name":"{}","model":"XIAO ESP32C6 (ESP-NOW)","manufacturer":"Climate Monitor"}}"#,
            s.base_topic,
            device_id,
            crate::json_escape(device_name.as_str())
        );
        // Mark entities unavailable if the sensor misses ~3 reports.
        let expire = if r.interval_s > 0 {
            format!(r#","expire_after":{}"#, r.interval_s as u32 * 3 + 60)
        } else {
            String::new()
        };
        for e in ENTITIES.iter() {
            let config_topic = format!(
                "{}/sensor/{}_{}/config",
                crate::HA_DISCOVERY_PREFIX, device_id, e.key
            );
            let payload = format!(
                r#"{{"name":"{}","unique_id":"{}_{}_{}","state_topic":"{}","unit_of_measurement":"{}","device_class":"{}","state_class":"measurement"{}{},"device":{}}}"#,
                e.name,
                s.base_topic,
                device_id,
                e.key,
                crate::json_escape(topic(e.key).as_str()),
                e.unit,
                e.device_class,
                if e.diagnostic { r#","entity_category":"diagnostic""# } else { "" },
                expire,
                device_block,
            );
            let _ = with_io_timeout(crate::mqtt_publish(
                &mut socket,
                config_topic.as_str(),
                payload.as_bytes(),
                true,
            ))
            .await;
        }
    }

    let mut values: [(&str, Option<String>); 5] = [
        ("temperature", None),
        ("humidity", None),
        ("battery", None),
        ("battery_voltage", None),
        ("rssi", Some(format!("{}", item.rssi))),
    ];
    if r.sensor_ok() {
        values[0].1 = Some(format!("{:.2}", r.temperature_c()));
        values[1].1 = Some(format!("{:.2}", r.humidity_pct()));
    }
    if r.battery_mv > 0 {
        values[2].1 = Some(format!("{}", espnow_packet::lipo_percent(r.battery_mv)));
        values[3].1 = Some(format!("{:.2}", r.battery_mv as f32 / 1000.0));
    }
    for (key, value) in values.iter() {
        if let Some(value) = value {
            let _ = with_io_timeout(crate::mqtt_publish(
                &mut socket,
                topic(key).as_str(),
                value.as_bytes(),
                true,
            ))
            .await;
        }
    }

    let _ = with_io_timeout(crate::mqtt_disconnect(&mut socket)).await;
    socket.close();
    let _ = with_io_timeout(socket.flush()).await;
    true
}
