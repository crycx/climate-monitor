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
//! The gateway is enabled only with an ESP-NOW key in the settings
//! (`espnow_key`, same value as on the sensors; `ESPNOW_KEY` at build time
//! is its default). It must stay awake to receive, so deep
//! sleep should be disabled on the gateway.

use alloc::{format, string::String, vec::Vec};
use core::cell::RefCell;
use core::sync::atomic::{AtomicU32, Ordering};

use embassy_net::{Stack, tcp::{TcpSocket, client::TcpClientState}, IpEndpoint};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, mutex::Mutex};
use embassy_time::{Duration, Instant};
use esp_radio::esp_now::{EspNow, EspNowWifiInterface, PeerInfo};

use crate::app_config::AppConfig;
use crate::espnow_packet::{self, Ack, ConfigMsg, DecodeError, Packet, Reading};
use crate::log_runtime_line;

const QUEUE_DEPTH: usize = 8;
const MAX_TRACKED_SENSORS: usize = 16;

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

/// One sensor for a table view (Nextion): every sensor heard since boot.
pub struct SensorRow {
    pub name: String,
    pub temp: Option<f32>,
    pub hum: Option<f32>,
    pub battery_pct: Option<u8>,
    /// On USB power: battery % would only show the charge voltage.
    pub charging: bool,
    pub age_secs: u64,
    /// Missed about three reports.
    pub stale: bool,
}

/// Sensors heard since boot, in stable slot order.
pub fn sensor_rows() -> Vec<SensorRow> {
    let now = Instant::now();
    critical_section::with(|cs| {
        DISPLAY
            .borrow_ref(cs)
            .iter()
            .flatten()
            .map(|s| {
                let r = &s.item.reading;
                let name = r.name().trim();
                let ok = r.sensor_ok();
                let age_secs = now.duration_since(s.received_at).as_secs();
                SensorRow {
                    name: if name.is_empty() {
                        format!("Sensor {}", &mac_hex(&s.item.mac)[6..])
                    } else {
                        name.into()
                    },
                    temp: ok.then(|| r.temperature_c()),
                    hum: ok.then(|| r.humidity_pct()),
                    battery_pct: (r.battery_mv > 0 && !r.usb_power())
                        .then(|| espnow_packet::lipo_percent(r.battery_mv)),
                    charging: r.usb_power(),
                    age_secs,
                    stale: age_secs > stale_after_secs(r),
                }
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
                // On USB the charge voltage says nothing about the level: show "USB" instead.
                battery_pct: (r.battery_mv > 0 && !r.usb_power())
                    .then(|| espnow_packet::lipo_percent(r.battery_mv)),
                footer: format!(
                    "{}  {}  {}dBm",
                    if r.usb_power() { "USB" } else { "ESP-NOW" },
                    format_age(now.duration_since(s.received_at).as_secs()),
                    s.item.rssi
                ),
            }
        })
        .collect();
    (pages, newest)
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

/// Answers every reading with the pending remote config (`set = 0` if none),
/// which also tells the sensor this gateway supports remote config.
async fn send_config(esp_now: &mut EspNow<'static>, key: &[u8], own_mac: &[u8; 6], dst: &[u8; 6], r: &Reading) {
    let d = desired_for(dst);
    let mut msg = ConfigMsg {
        boot_id: r.boot_id,
        seq: r.seq,
        caps: espnow_packet::CAP_VERSION,
        ..ConfigMsg::default()
    };
    if let Some(v) = d.temp {
        msg.set |= espnow_packet::SET_TEMP_OFFSET;
        msg.temp_offset_centi = v;
    }
    if let Some(v) = d.hum {
        msg.set |= espnow_packet::SET_HUM_OFFSET;
        msg.hum_offset_centi = v;
    }
    if let Some(v) = d.interval {
        msg.set |= espnow_packet::SET_INTERVAL;
        msg.interval_s = v;
    }
    if crate::gateway_ota::offer(dst, r) {
        msg.set |= espnow_packet::SET_OTA;
    }
    if msg.set != 0 {
        log_runtime_line(format!("ESP-NOW {}: sending remote config (set 0x{:02x})", mac_hex(dst), msg.set).as_str());
    }
    let mut buf = [0u8; espnow_packet::MAX_PACKET_LEN];
    let len = espnow_packet::encode_config(key, own_mac, &msg, &mut buf);

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
                log_runtime_line(format!("ESP-NOW: add_peer for config failed: {:?}", e).as_str());
                return;
            }
        }
    };

    if let Err(e) = esp_now.send_async(dst, &buf[..len]).await {
        log_runtime_line(format!("ESP-NOW: config to {} failed: {:?}", mac_hex(dst), e).as_str());
    }

    if added {
        let _ = esp_now.remove_peer(dst);
    }
}

/// Unicast to `dst`; like the ack, the peer entry is removed afterwards.
async fn send_unicast(esp_now: &mut EspNow<'static>, dst: &[u8; 6], data: &[u8], what: &str) {
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
                log_runtime_line(format!("ESP-NOW: add_peer for {} failed: {:?}", what, e).as_str());
                return;
            }
        }
    };
    if let Err(e) = esp_now.send_async(dst, data).await {
        log_runtime_line(format!("ESP-NOW: {} to {} failed: {:?}", what, mac_hex(dst), e).as_str());
    }
    if added {
        let _ = esp_now.remove_peer(dst);
    }
}

#[embassy_executor::task]
pub async fn rx_task(mut esp_now: EspNow<'static>, own_mac: [u8; 6], key: String) {
    let key = key.as_bytes();
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
                send_config(&mut esp_now, key, &own_mac, &src, &reading).await;
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
            Ok(Packet::OtaRequest { offset }) => {
                if let Some(reply) = crate::gateway_ota::serve(&src, offset) {
                    let mut buf = [0u8; espnow_packet::MAX_PACKET_LEN];
                    let len = match &reply {
                        crate::gateway_ota::Reply::Info(info) => {
                            espnow_packet::encode_ota_info(key, &own_mac, info, &mut buf)
                        }
                        crate::gateway_ota::Reply::Data { offset, len, data } => {
                            espnow_packet::encode_ota_data(key, &own_mac, *offset, &data[..*len], &mut buf)
                        }
                    };
                    send_unicast(&mut esp_now, &src, &buf[..len], "OTA").await;
                }
            }
            // Ack, Config and OTA replies are for sensors, not for us.
            Ok(_) => {}
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

/// Remote config HA wants for a sensor (retained `<base>/<device>/set/<field>`
/// commands), loaded on each MQTT session and sent in the Config reply to the
/// sensor's next reading. The gateway's own sensor uses it too (main.rs).
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Desired {
    /// degC * 100
    pub(crate) temp: Option<i16>,
    /// % * 100
    pub(crate) hum: Option<i16>,
    /// seconds
    pub(crate) interval: Option<u16>,
    /// HA's update entity asked to install the published firmware.
    pub(crate) ota: bool,
}

impl Desired {
    pub(crate) fn is_empty(&self) -> bool {
        self.temp.is_none() && self.hum.is_none() && self.interval.is_none() && !self.ota
    }
}

static DESIRED: critical_section::Mutex<RefCell<[Option<([u8; 6], Desired)>; MAX_TRACKED_SENSORS]>> =
    critical_section::Mutex::new(RefCell::new([None; MAX_TRACKED_SENSORS]));

fn desired_for(mac: &[u8; 6]) -> Desired {
    critical_section::with(|cs| {
        DESIRED
            .borrow_ref(cs)
            .iter()
            .flatten()
            .find(|(m, _)| m == mac)
            .map(|(_, d)| *d)
            .unwrap_or_default()
    })
}

fn set_desired(mac: &[u8; 6], d: Desired) {
    critical_section::with(|cs| {
        let mut table = DESIRED.borrow_ref_mut(cs);
        if let Some(slot) = table.iter_mut().find(|e| matches!(e, Some((m, _)) if m == mac)) {
            *slot = if d.is_empty() { None } else { Some((*mac, d)) };
        } else if !d.is_empty() {
            match table.iter_mut().find(|e| e.is_none()) {
                Some(slot) => *slot = Some((*mac, d)),
                None => table[0] = Some((*mac, d)),
            }
        }
    });
}

/// HA `number` entities for remote config: (key, name, unit, min, max, step, extra JSON).
pub(crate) const CONFIG_NUMBERS: [(&str, &str, &str, &str, &str, &str, &str); 3] = [
    ("temp_offset", "Temperature offset", "°C", "-20", "20", "0.1", r#","icon":"mdi:thermometer-plus""#),
    ("hum_offset", "Humidity offset", "%", "-20", "20", "0.1", r#","icon":"mdi:water-plus""#),
    ("interval", "Interval", "s", "10", "21600", "10", r#","device_class":"duration""#),
];

/// Centi-units from an HA number command ("-0.5", "300", "300.0").
fn parse_centi(payload: &[u8], limit: f32) -> Option<i32> {
    let v: f32 = core::str::from_utf8(payload).ok()?.trim().parse().ok()?;
    if !v.is_finite() || v.abs() > limit {
        return None;
    }
    let scaled = v * 100.0;
    Some((if scaled >= 0.0 { scaled + 0.5 } else { scaled - 0.5 }) as i32)
}

async fn read_exact_within(socket: &mut TcpSocket<'_>, buf: &mut [u8], timeout: Duration) -> Option<()> {
    let mut off = 0;
    while off < buf.len() {
        let n = embassy_time::with_timeout(timeout, socket.read(&mut buf[off..])).await.ok()?.ok()?;
        if n == 0 {
            return None;
        }
        off += n;
    }
    Some(())
}

/// Reads one MQTT packet: (fixed header byte, body length in `body`).
/// `first_byte_wait` bounds the idle wait; None on timeout, close or a body
/// larger than `body` (we only expect tiny packets here).
async fn mqtt_read_packet(
    socket: &mut TcpSocket<'_>,
    body: &mut [u8],
    first_byte_wait: Duration,
) -> Option<(u8, usize)> {
    let io = Duration::from_secs(crate::MQTT_IO_TIMEOUT_SECS);
    let mut b = [0u8; 1];
    read_exact_within(socket, &mut b, first_byte_wait).await?;
    let kind = b[0];
    let mut len = 0usize;
    let mut shift = 0;
    loop {
        read_exact_within(socket, &mut b, io).await?;
        len |= ((b[0] & 0x7f) as usize) << shift;
        if b[0] & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift > 21 {
            return None;
        }
    }
    if len > body.len() {
        return None;
    }
    read_exact_within(socket, &mut body[..len], io).await?;
    Some((kind, len))
}

/// Subscribes to `<base>/<device>/set/+` and collects the retained commands
/// HA left there. None if the broker did not answer the subscribe.
///
/// With `meta_topic` it also reads a published firmware's description
/// (None: nothing published).
pub(crate) async fn read_desired(
    socket: &mut TcpSocket<'_>,
    filter: &str,
    meta_topic: Option<&str>,
) -> Option<(Desired, Option<espnow_packet::OtaInfo>)> {
    let filters: Vec<&[u8]> = core::iter::once(filter).chain(meta_topic).map(str::as_bytes).collect();
    let mut packet: Vec<u8> = Vec::with_capacity(64 + filter.len());
    packet.push(0x82); // SUBSCRIBE
    crate::encode_remaining_length(&mut packet, 2 + filters.iter().map(|f| 2 + f.len() + 1).sum::<usize>());
    packet.extend_from_slice(&[0x00, 0x01]); // packet id 1
    for f in filters.iter() {
        packet.push((f.len() >> 8) as u8);
        packet.push(f.len() as u8);
        packet.extend_from_slice(f);
        packet.push(0x00); // QoS 0
    }
    crate::socket_write_all(socket, &packet).await.ok()?;

    let mut desired = Desired::default();
    let mut meta = None;
    let mut subscribed = false;
    let mut body = [0u8; 256];
    // Retained messages arrive right around the SUBACK: stop after a short idle.
    loop {
        let wait = if subscribed {
            Duration::from_millis(300)
        } else {
            Duration::from_secs(crate::MQTT_IO_TIMEOUT_SECS)
        };
        let Some((kind, len)) = mqtt_read_packet(socket, &mut body, wait).await else {
            break;
        };
        match kind >> 4 {
            9 => subscribed = true,
            3 if len >= 2 => {
                let topic_len = ((body[0] as usize) << 8) | body[1] as usize;
                // QoS 0 delivery: no packet id between topic and payload.
                if 2 + topic_len > len || kind & 0x06 != 0 {
                    continue;
                }
                let topic = core::str::from_utf8(&body[2..2 + topic_len]).unwrap_or("");
                let payload = &body[2 + topic_len..len];
                if Some(topic) == meta_topic {
                    meta = crate::gateway_ota::parse_meta(payload);
                    continue;
                }
                match topic.rsplit('/').next().unwrap_or("") {
                    "ota" => desired.ota = payload == b"install",
                    "temp_offset" => desired.temp = parse_centi(payload, 20.0).map(|v| v as i16),
                    "hum_offset" => desired.hum = parse_centi(payload, 20.0).map(|v| v as i16),
                    "interval" => {
                        desired.interval = parse_centi(payload, 21600.0)
                            .map(|v| (v + 50) / 100)
                            .filter(|s| (10..=21600).contains(s))
                            .map(|s| s as u16)
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    subscribed.then_some((desired, meta))
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
    let mut battery_full = espnow_packet::BatteryFullTracker::<MAX_TRACKED_SENSORS>::new();
    loop {
        let item = READINGS.receive().await;
        let r = &item.reading;
        let mac = mac_hex(&item.mac);
        // Tracked even while MQTT is down, so the hour keeps counting.
        let full = battery_full.update(&item.mac, r, Instant::now().as_secs());
        if r.sensor_ok() {
            log_runtime_line(
                format!(
                    "ESP-NOW {} '{}': {:.1} C {:.1} % bat {} mV{} rssi {} seq {}",
                    mac,
                    r.name(),
                    r.temperature_c(),
                    r.humidity_pct(),
                    r.battery_mv,
                    if r.usb_power() { " (USB, charging)" } else { "" },
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
            // Re-announce once the sensor starts reporting config (adds the
            // numbers) and when its firmware version changes (sw_version).
            let name_hash = fnv1a(r.name())
                ^ if r.has_config() { 0x9E37_79B9 } else { 0 }
                ^ fnv1a(r.version()).rotate_left(7)
                ^ if r.ota_capable() { 0x5BD1_E995 } else { 0 };
            let needs_discovery = !announced.contains(&item.mac, name_hash);
            if publish(*stack, &settings, &item, needs_discovery, full).await && needs_discovery {
                announced.insert(&item.mac, name_hash);
            }
            // "Install" from HA's update entity (read in that session).
            let d = desired_for(&item.mac);
            if d.ota {
                set_desired(&item.mac, Desired { ota: false, ..d });
                if r.ota_capable() {
                    let broker = crate::gateway_ota::Broker {
                        host: settings.host.as_str(),
                        port: settings.port,
                        username: settings.username.as_str(),
                        password: settings.password.as_str(),
                        base: settings.base_topic.as_str(),
                    };
                    crate::gateway_ota::install(*stack, &broker, &item.mac, r.version()).await;
                } else {
                    log_runtime_line(
                        format!("OTA {}: this sensor's firmware cannot update over the air (flash it over USB once)", mac)
                            .as_str(),
                    );
                }
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

/// Below this the "Battery low" binary sensor turns on (about 10 % on the
/// LiPo curve in `espnow_packet::lipo_percent`): time to charge.
const LOW_BATTERY_MV: u16 = 3600;

struct EntitySpec {
    key: &'static str,
    name: &'static str,
    unit: &'static str,
    device_class: &'static str,
    diagnostic: bool,
    /// HA `suggested_display_precision` (decimals shown); HA would otherwise
    /// show volts as a whole number ("4 V").
    precision: u8,
}

const ENTITIES: [EntitySpec; 5] = [
    EntitySpec { key: "temperature", name: "Temperature", unit: "°C", device_class: "temperature", diagnostic: false, precision: 1 },
    EntitySpec { key: "humidity", name: "Humidity", unit: "%", device_class: "humidity", diagnostic: false, precision: 0 },
    EntitySpec { key: "battery", name: "Battery", unit: "%", device_class: "battery", diagnostic: true, precision: 0 },
    EntitySpec { key: "battery_voltage", name: "Battery voltage", unit: "V", device_class: "voltage", diagnostic: true, precision: 2 },
    EntitySpec { key: "rssi", name: "Signal strength", unit: "dBm", device_class: "signal_strength", diagnostic: true, precision: 0 },
];

/// Publishes one reading; returns true if the broker accepted the session.
async fn publish(
    stack: Stack<'static>,
    s: &MqttSettings,
    item: &RemoteReading,
    discovery: bool,
    battery_full: bool,
) -> bool {
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

    // Remote config: pick up HA's retained commands for this sensor; the
    // Config reply to its next reading carries them.
    let meta_topic = crate::gateway_ota::meta_topic(s.base_topic.as_str());
    if let Some((d, meta)) = read_desired(&mut socket, topic("set/+").as_str(), Some(meta_topic.as_str())).await {
        crate::gateway_ota::set_latest(meta);
        set_desired(&item.mac, d);
    }

    if discovery {
        let name = r.name().trim();
        let device_name = if name.is_empty() {
            format!("Climate Sensor {}", &device_id[device_id.len() - 6..])
        } else {
            format!("Climate Sensor ({})", name)
        };
        let sw_version = if r.version().is_empty() {
            String::new()
        } else {
            format!(r#","sw_version":"{}""#, crate::json_escape(r.version()))
        };
        let device_block = format!(
            r#"{{"identifiers":["{}:{}"],"name":"{}","model":"XIAO ESP32C6 (ESP-NOW)","manufacturer":"Climate Monitor"{}}}"#,
            s.base_topic,
            device_id,
            crate::json_escape(device_name.as_str()),
            sw_version
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
                r#"{{"name":"{}","unique_id":"{}_{}_{}","state_topic":"{}","unit_of_measurement":"{}","device_class":"{}","state_class":"measurement","suggested_display_precision":{}{}{},"device":{}}}"#,
                e.name,
                s.base_topic,
                device_id,
                e.key,
                crate::json_escape(topic(e.key).as_str()),
                e.unit,
                e.device_class,
                e.precision,
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
        // Binary "charge me" warning; on = battery low (device_class battery).
        let config_topic = format!(
            "{}/binary_sensor/{}_battery_low/config",
            crate::HA_DISCOVERY_PREFIX, device_id
        );
        let payload = format!(
            r#"{{"name":"Battery low","unique_id":"{}_{}_battery_low","state_topic":"{}","device_class":"battery","payload_on":"ON","payload_off":"OFF"{},"device":{}}}"#,
            s.base_topic,
            device_id,
            crate::json_escape(topic("battery_low").as_str()),
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
        // No expire_after: "Last seen" should keep showing when it last reported.
        let payload = crate::last_seen_discovery(
            format!("{}_{}_last_seen", s.base_topic, device_id).as_str(),
            topic("last_seen").as_str(),
            device_block.as_str(),
        );
        let _ = with_io_timeout(crate::mqtt_publish(
            &mut socket,
            format!("{}/sensor/{}_last_seen/config", crate::HA_DISCOVERY_PREFIX, device_id).as_str(),
            payload.as_bytes(),
            true,
        ))
        .await;
        // On = sensor on USB power, cell charging (battery % is then not the charge level).
        let config_topic = format!(
            "{}/binary_sensor/{}_charging/config",
            crate::HA_DISCOVERY_PREFIX, device_id
        );
        let payload = format!(
            r#"{{"name":"Charging","unique_id":"{}_{}_charging","state_topic":"{}","device_class":"battery_charging","payload_on":"ON","payload_off":"OFF"{},"device":{}}}"#,
            s.base_topic,
            device_id,
            crate::json_escape(topic("charging").as_str()),
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
        // On = charged: on USB at ~4.2 V for an hour (`espnow_packet::BatteryFullTracker`).
        let config_topic = format!(
            "{}/binary_sensor/{}_battery_full/config",
            crate::HA_DISCOVERY_PREFIX, device_id
        );
        let payload = format!(
            r#"{{"name":"Battery full","unique_id":"{}_{}_battery_full","state_topic":"{}","icon":"mdi:battery-check","payload_on":"ON","payload_off":"OFF"{},"device":{}}}"#,
            s.base_topic,
            device_id,
            crate::json_escape(topic("battery_full").as_str()),
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
        if r.has_config() {
            // Remote config as HA numbers: HA publishes changes retained to
            // set/<key>; the state is what the sensor reports it uses.
            for (key, name, unit, min, max, step, extra) in CONFIG_NUMBERS.iter() {
                let config_topic = format!(
                    "{}/number/{}_{}/config",
                    crate::HA_DISCOVERY_PREFIX, device_id, key
                );
                let payload = format!(
                    r#"{{"name":"{}","unique_id":"{}_{}_{}","command_topic":"{}","state_topic":"{}","min":{},"max":{},"step":{},"mode":"box","unit_of_measurement":"{}","retain":true,"entity_category":"config"{},"device":{}}}"#,
                    name,
                    s.base_topic,
                    device_id,
                    key,
                    crate::json_escape(topic(format!("set/{}", key).as_str()).as_str()),
                    crate::json_escape(topic(key).as_str()),
                    min,
                    max,
                    step,
                    unit,
                    extra,
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
        if r.ota_capable() {
            // Firmware update (gateway_ota): "Install" leaves a retained
            // command on set/ota; the state shows installed vs. published.
            let config_topic = format!("{}/update/{}_firmware/config", crate::HA_DISCOVERY_PREFIX, device_id);
            let payload = format!(
                r#"{{"name":"Firmware","unique_id":"{}_{}_firmware","state_topic":"{}","command_topic":"{}","payload_install":"install","retain":true,"device_class":"firmware","entity_category":"config","device":{}}}"#,
                s.base_topic,
                device_id,
                crate::json_escape(topic("firmware").as_str()),
                crate::json_escape(topic("set/ota").as_str()),
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

    let mut values: [(&str, Option<String>); 8] = [
        ("temperature", None),
        ("humidity", None),
        ("battery", None),
        ("battery_voltage", None),
        ("battery_low", None),
        ("charging", Some(String::from(if r.usb_power() { "ON" } else { "OFF" }))),
        ("battery_full", Some(String::from(if battery_full { "ON" } else { "OFF" }))),
        ("rssi", Some(format!("{}", item.rssi))),
    ];
    if r.sensor_ok() {
        values[0].1 = Some(format!("{:.2}", r.temperature_c()));
        values[1].1 = Some(format!("{:.2}", r.humidity_pct()));
    }
    if r.battery_mv > 0 {
        values[2].1 = Some(format!("{}", espnow_packet::lipo_percent(r.battery_mv)));
        values[3].1 = Some(format!("{:.2}", r.battery_mv as f32 / 1000.0));
        // While charging the cell reads ~4.2 V anyway; never warn then.
        let low = r.battery_mv < LOW_BATTERY_MV && !r.usb_power();
        values[4].1 = Some(String::from(if low { "ON" } else { "OFF" }));
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
    if r.has_config() {
        let states = [
            ("temp_offset", format!("{:.1}", r.temp_offset_centi as f32 / 100.0)),
            ("hum_offset", format!("{:.1}", r.hum_offset_centi as f32 / 100.0)),
            ("interval", format!("{}", r.interval_s)),
        ];
        for (key, value) in states.iter() {
            let _ = with_io_timeout(crate::mqtt_publish(&mut socket, topic(key).as_str(), value.as_bytes(), true)).await;
        }
        // A command the sensor now reports as applied is done: clear the
        // retained command, so a later change made on the sensor itself
        // (console, portal) is not overwritten again.
        let mut d = desired_for(&item.mac);
        let mut done: [Option<&str>; 3] = [None; 3];
        if d.temp == Some(r.temp_offset_centi) {
            d.temp = None;
            done[0] = Some("temp_offset");
        }
        if d.hum == Some(r.hum_offset_centi) {
            d.hum = None;
            done[1] = Some("hum_offset");
        }
        if d.interval == Some(r.interval_s) {
            d.interval = None;
            done[2] = Some("interval");
        }
        for key in done.iter().flatten() {
            let _ = with_io_timeout(crate::mqtt_publish(
                &mut socket,
                topic(format!("set/{}", key).as_str()).as_str(),
                b"",
                true,
            ))
            .await;
            log_runtime_line(format!("ESP-NOW {}: remote {} applied", mac_hex(&item.mac), key).as_str());
        }
        set_desired(&item.mac, d);
    }
    if r.ota_capable() {
        let state = crate::gateway_ota::ha_state(&item.mac, r.version());
        let _ = with_io_timeout(crate::mqtt_publish(&mut socket, topic("firmware").as_str(), state.as_bytes(), true)).await;
    }
    if desired_for(&item.mac).ota {
        // One click, one attempt: forward_task acts on it after this session.
        let _ = with_io_timeout(crate::mqtt_publish(&mut socket, topic("set/ota").as_str(), b"", true)).await;
    }
    // Not retained: HA stamps "Last seen" with the arrival time (see last_seen_discovery).
    let _ = with_io_timeout(crate::mqtt_publish(&mut socket, topic("last_seen").as_str(), b"1", false)).await;

    let _ = with_io_timeout(crate::mqtt_disconnect(&mut socket)).await;
    socket.close();
    let _ = with_io_timeout(socket.flush()).await;
    true
}
