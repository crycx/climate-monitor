//! ESP-NOW transport (`transport = espnow` in the portal).
//!
//! No WiFi association, DHCP or TLS: each cycle reads the DHT22, shows it on
//! the OLED and sends one authenticated ESP-NOW packet to the gateway
//! (`esp32/c6` firmware), which forwards it to Home Assistant over MQTT.
//!
//! - Deep sleep enabled (and power source = battery): measure, send, sleep.
//!   On power-on / RESET the device first waits 3 s so the button can open
//!   the portal.
//! - Otherwise it stays awake and sends every interval.
//!
//! Pressing the button restarts into the regular WiFi runtime with the AP
//! portal open (see `take_portal_request`), where the transport can be
//! switched back to WiFi.

use alloc::{format, string::String};
use core::sync::atomic::Ordering;
use core::time::Duration as CoreDuration;

use display_interface::WriteOnlyDataCommand;
use embassy_time::{Duration, Instant, Timer, with_timeout};
use embedded_dht_rs::dht22::Dht22;
use esp_hal::{
    delay::Delay,
    gpio::{DriveMode, Flex, OutputConfig, Pull},
    peripherals::{GPIO5, LPWR, WIFI},
    rng::Rng,
    rtc_cntl::{Rtc, SocResetReason, sleep::TimerWakeupSource},
};
use esp_radio::Controller;
use esp_radio::esp_now::{BROADCAST_ADDRESS, EspNow, EspNowWifiInterface, PeerInfo};
use esp_radio::wifi::{WifiController, WifiMode};
use static_cell::StaticCell;

use crate::app_config::{AppConfig, ESPNOW_KEY_MIN_LEN};
use crate::espnow_packet::{self, Kind, MAX_PACKET_LEN, Packet, Reading};
use crate::oled_display::{self, OledDisplay};
use crate::{
    BOOTING, BUTTON_PRESSED, LAST_SENT_SECS, POWER_SAVE_ACTIVE, SENDING, WIFI_CONNECTING, draw_status_for_mode,
    is_power_bank_mode, log_runtime_line, sensor_interval_secs,
};

/// Cold-boot window for the button before the first deep sleep.
const PORTAL_WINDOW_MS: u64 = 3000;
const BUTTON_POLL_MS: u64 = 50;
/// DHT22 needs >= 2 s between conversions.
const DHT_RETRY_DELAY_MS: u64 = 2100;
const DHT_ATTEMPTS: u8 = 2;
const UNICAST_ATTEMPTS: u8 = 3;
/// The gateway answers from the executor that also runs its HTTPS work.
const ACK_WAIT_MS: u64 = 150;
const MAX_CHANNEL: u8 = 13;

// ── State kept in RTC fast memory ───────────────────────────────────────────
// `persistent` statics survive deep sleep and software resets, and are
// zeroed on power-on only.

const STATE_MAGIC: u32 = 0x434D_4333; // "CMC3"
const PORTAL_REQUEST_MAGIC: u32 = 0x504F_5254; // "PORT"

#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut RTC_MAGIC: u32 = 0;
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut RTC_BOOT_ID: u32 = 0;
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut RTC_SEQ: u32 = 0;
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut RTC_CHANNEL: u8 = 0;
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut RTC_GATEWAY: [u8; 6] = [0; 6];
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut RTC_PORTAL_REQUEST: u32 = 0;

/// True once after `request_portal_and_reset`; the caller then opens the portal.
pub fn take_portal_request() -> bool {
    // SAFETY: only called from the main task during boot.
    unsafe {
        let requested = RTC_PORTAL_REQUEST == PORTAL_REQUEST_MAGIC;
        RTC_PORTAL_REQUEST = 0;
        requested
    }
}

fn request_portal_and_reset() -> ! {
    log_runtime_line("ESP-NOW: button pressed, restarting into the AP portal...");
    // SAFETY: we reset right after; nothing else touches this static.
    unsafe {
        RTC_PORTAL_REQUEST = PORTAL_REQUEST_MAGIC;
    }
    esp_hal::system::software_reset()
}

struct NodeState {
    boot_id: u32,
    seq: u32,
    /// 0 = gateway unknown, search on next send.
    channel: u8,
    gateway: [u8; 6],
}

impl NodeState {
    fn load_or_init() -> Self {
        // SAFETY: only the main task touches these, never concurrently.
        unsafe {
            if RTC_MAGIC == STATE_MAGIC {
                return Self {
                    boot_id: RTC_BOOT_ID,
                    seq: RTC_SEQ,
                    channel: RTC_CHANNEL,
                    gateway: RTC_GATEWAY,
                };
            }
        }
        // Fresh boot_id lets the gateway accept the restarted counter.
        Self {
            boot_id: Rng::new().random(),
            seq: 0,
            channel: 0,
            gateway: [0; 6],
        }
    }

    fn store(&self) {
        // SAFETY: see load_or_init.
        unsafe {
            RTC_BOOT_ID = self.boot_id;
            RTC_SEQ = self.seq;
            RTC_CHANNEL = self.channel;
            RTC_GATEWAY = self.gateway;
            RTC_MAGIC = STATE_MAGIC;
        }
    }

    fn next_seq(&mut self) -> u32 {
        self.seq = self.seq.wrapping_add(1);
        self.seq
    }
}

// ── Radio ───────────────────────────────────────────────────────────────────

static RADIO_CONTROLLER: StaticCell<Controller<'static>> = StaticCell::new();

struct Radio {
    _controller: WifiController<'static>,
    esp_now: EspNow<'static>,
    own_mac: [u8; 6],
}

impl Radio {
    /// Starts the radio in unconnected station mode, which ESP-NOW needs.
    fn start(wifi: WIFI<'static>) -> Self {
        let radio = RADIO_CONTROLLER.init(esp_radio::init().expect("radio init failed"));
        let (mut controller, interfaces) =
            esp_radio::wifi::new(radio, wifi, Default::default()).expect("WiFi init failed");
        controller.set_mode(WifiMode::Sta).expect("WiFi STA mode failed");
        controller.start().expect("WiFi start failed");
        Self {
            own_mac: interfaces.sta.mac_address(),
            esp_now: interfaces.esp_now,
            _controller: controller,
        }
    }
}

fn add_unicast_peer(esp_now: &EspNow<'static>, mac: &[u8; 6]) -> bool {
    esp_now.peer_exists(mac)
        || esp_now
            .add_peer(PeerInfo {
                interface: EspNowWifiInterface::Sta,
                peer_address: *mac,
                lmk: None,
                channel: None,
                encrypt: false,
            })
            .is_ok()
}

async fn wait_for_ack(esp_now: &mut EspNow<'static>, key: &[u8], reading: &Reading) -> Option<[u8; 6]> {
    let deadline = Instant::now() + Duration::from_millis(ACK_WAIT_MS);
    loop {
        let remaining = deadline.checked_duration_since(Instant::now())?;
        let rx = with_timeout(remaining, esp_now.receive_async()).await.ok()?;
        let src = rx.info.src_address;
        if let Ok(Packet::Ack(ack)) = espnow_packet::decode(key, &src, rx.data()) {
            if ack.boot_id == reading.boot_id && ack.seq == reading.seq {
                return Some(src);
            }
        }
    }
}

/// Unicast to the known gateway, else broadcast a discover on each channel
/// (last known first) until the gateway acks. Updates `state` on success.
async fn deliver(radio: &mut Radio, key: &[u8], state: &mut NodeState, reading: &mut Reading) -> bool {
    let esp_now = &mut radio.esp_now;
    let own_mac = radio.own_mac;
    let mut buf = [0u8; MAX_PACKET_LEN];

    if state.channel != 0 {
        if esp_now.set_channel(state.channel).is_ok() && add_unicast_peer(esp_now, &state.gateway) {
            reading.seq = state.next_seq();
            let len = espnow_packet::encode_reading(key, &own_mac, Kind::Reading, reading, &mut buf);
            for attempt in 1..=UNICAST_ATTEMPTS {
                match esp_now.send_async(&state.gateway, &buf[..len]).await {
                    // The MAC-layer ACK confirms delivery.
                    Ok(()) => return true,
                    Err(e) => {
                        log_runtime_line(
                            format!("ESP-NOW: unicast attempt {}/{} failed: {:?}", attempt, UNICAST_ATTEMPTS, e)
                                .as_str(),
                        );
                        Timer::after(Duration::from_millis(15 * attempt as u64)).await;
                    }
                }
            }
        }
        log_runtime_line(
            format!("ESP-NOW: gateway not reachable on channel {}, searching...", state.channel).as_str(),
        );
    }

    reading.seq = state.next_seq();
    let len = espnow_packet::encode_reading(key, &own_mac, Kind::Discover, reading, &mut buf);
    let hint = state.channel;
    let channels = core::iter::once(hint)
        .filter(|ch| *ch != 0)
        .chain((1..=MAX_CHANNEL).filter(move |ch| *ch != hint));
    for channel in channels {
        if esp_now.set_channel(channel).is_err() {
            continue;
        }
        if esp_now.send_async(&BROADCAST_ADDRESS, &buf[..len]).await.is_err() {
            continue;
        }
        if let Some(gateway) = wait_for_ack(esp_now, key, reading).await {
            log_runtime_line(
                format!("ESP-NOW: gateway {} found on channel {}", mac_str(&gateway), channel).as_str(),
            );
            state.channel = channel;
            state.gateway = gateway;
            return true;
        }
    }
    // Keep the old hint: the gateway may just be rebooting.
    false
}

// ── Measurement ─────────────────────────────────────────────────────────────

fn read_dht22(pin: &mut Flex<'static>) -> Option<(f32, f32)> {
    // Bit timing is tens of µs: keep radio and scheduler interrupts out of the way.
    critical_section::with(|_| Dht22::new(&mut *pin, Delay::new()).read())
        .map(|r| (r.temperature, r.humidity))
        .map_err(|e| log_runtime_line(format!("DHT22 read error: {:?}", e).as_str()))
        .ok()
}

async fn measure(pin: &mut Flex<'static>, cfg: &AppConfig) -> Option<(f32, f32)> {
    let temp_offset: f32 = cfg.temp_offset.parse().unwrap_or(-1.7);
    let hum_offset: f32 = cfg.hum_offset.parse().unwrap_or(-7.0);
    for attempt in 1..=DHT_ATTEMPTS {
        if attempt > 1 {
            Timer::after(Duration::from_millis(DHT_RETRY_DELAY_MS)).await;
        }
        if let Some((t, h)) = read_dht22(pin) {
            let (t, h) = (t + temp_offset, (h + hum_offset).clamp(0.0, 100.0));
            log_runtime_line(
                format!(
                    "DHT22: {:.1} C  {:.1} %  (offsets: temp {:+.1} C, hum {:+.1} %)",
                    t, h, temp_offset, hum_offset
                )
                .as_str(),
            );
            return Some((t, h));
        }
    }
    None
}

fn build_reading(cfg: &AppConfig, state: &NodeState, climate: Option<(f32, f32)>) -> Reading {
    fn to_centi(value: f32) -> i32 {
        let scaled = value * 100.0;
        (if scaled >= 0.0 { scaled + 0.5 } else { scaled - 0.5 }) as i32
    }
    let mut reading = Reading::empty();
    reading.boot_id = state.boot_id;
    // No battery divider on the C3 boards: 0 = unknown.
    reading.battery_mv = 0;
    reading.interval_s = sensor_interval_secs(cfg).min(u16::MAX as u64) as u16;
    reading.set_name(cfg.espnow_display_name());
    match climate {
        Some((t, h)) => {
            reading.temp_centi = to_centi(t).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            reading.hum_centi = to_centi(h) as u16;
        }
        None => reading.flags |= espnow_packet::FLAG_SENSOR_ERROR,
    }
    reading
}

fn mac_str(mac: &[u8; 6]) -> String {
    format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    )
}

/// Waits `duration`, restarting into the portal if the button is pressed.
async fn wait_watching_button(duration: Duration) {
    let until = Instant::now() + duration;
    while Instant::now() < until {
        if BUTTON_PRESSED.load(Ordering::Relaxed) {
            request_portal_and_reset();
        }
        Timer::after(Duration::from_millis(BUTTON_POLL_MS)).await;
    }
}

// ── Main loop ───────────────────────────────────────────────────────────────

pub async fn run<DI>(
    cfg: AppConfig,
    wifi: WIFI<'static>,
    dht_gpio: GPIO5<'static>,
    mut oled: Option<&mut OledDisplay<DI>>,
    portrait: bool,
    lpwr: LPWR<'static>,
) -> !
where
    DI: WriteOnlyDataCommand,
{
    let cold_boot = esp_hal::system::reset_reason() != Some(SocResetReason::CoreDeepSleep);
    let deep_sleep = cfg.deep_sleep_enabled && !is_power_bank_mode(&cfg);
    let interval_secs = sensor_interval_secs(&cfg);
    let key = cfg.espnow_key.trim();
    let has_key = key.len() >= ESPNOW_KEY_MIN_LEN;

    BOOTING.store(false, Ordering::Relaxed);
    WIFI_CONNECTING.store(false, Ordering::Relaxed);
    POWER_SAVE_ACTIVE.store(false, Ordering::Relaxed);

    log_runtime_line(
        format!(
            "ESP-NOW mode: name '{}', interval {} s, {}",
            cfg.espnow_display_name(),
            interval_secs,
            if deep_sleep { "deep sleep" } else { "staying awake" }
        )
        .as_str(),
    );
    if !has_key {
        log_runtime_line("ESP-NOW: no key set; open the portal (button) and set the ESP-NOW key.");
    }

    // DHT22 on GPIO5 (open-drain, pull-up), same as the WiFi runtime.
    let mut dht_pin = Flex::new(dht_gpio);
    dht_pin.apply_output_config(&OutputConfig::default().with_drive_mode(DriveMode::OpenDrain).with_pull(Pull::Up));
    dht_pin.set_output_enable(true);
    dht_pin.set_input_enable(true);
    dht_pin.set_high();

    if cold_boot && deep_sleep {
        if let Some(display) = oled.as_deref_mut() {
            draw_status_for_mode(display, "ESP-NOW: BTN=setup", portrait);
        }
        log_runtime_line("ESP-NOW: press the button within 3 s to open the AP portal.");
        wait_watching_button(Duration::from_millis(PORTAL_WINDOW_MS)).await;
    }

    let mut radio = Radio::start(wifi);
    let mut state = NodeState::load_or_init();
    log_runtime_line(format!("ESP-NOW: own MAC {}", mac_str(&radio.own_mac)).as_str());

    let mut lpwr = Some(lpwr);
    loop {
        let cycle_started = Instant::now();
        SENDING.store(true, Ordering::Relaxed);
        let climate = measure(&mut dht_pin, &cfg).await;

        let status: String = if !has_key {
            "ESP-NOW: no key".into()
        } else {
            let mut reading = build_reading(&cfg, &state, climate);
            let delivered = deliver(&mut radio, key.as_bytes(), &mut state, &mut reading).await;
            state.store();
            log_runtime_line(
                format!(
                    "ESP-NOW: {} (seq {})",
                    if delivered { "delivered" } else { "gateway not found" },
                    reading.seq
                )
                .as_str(),
            );
            if delivered {
                LAST_SENT_SECS.store((Instant::now().as_millis() / 1000) as usize, Ordering::Relaxed);
                format!("ESP-NOW ch{}", state.channel)
            } else {
                "ESP-NOW: no gateway".into()
            }
        };
        SENDING.store(false, Ordering::Relaxed);

        if let Some(display) = oled.as_deref_mut() {
            let tick = (Instant::now().as_millis() / 1000) as usize;
            let (temp, hum) = climate.map_or((None, None), |(t, h)| (Some(t), Some(h)));
            let _ = oled_display::show_reading_with_wifi(display, temp, hum, status.as_str(), false, tick, portrait);
        }

        if deep_sleep && crate::usb_console::usb_host_present() {
            log_runtime_line("Power-save: computer on USB, staying awake for the console.");
        } else if deep_sleep {
            // Short look at the button before sleeping (also lets the log flush).
            wait_watching_button(Duration::from_millis(300)).await;
            log_runtime_line(
                format!(
                    "Power-save: awake {} ms, deep sleep {} s.",
                    cycle_started.elapsed().as_millis(),
                    interval_secs
                )
                .as_str(),
            );
            if let Some(display) = oled.as_deref_mut() {
                if let Err(e) = oled_display::prepare_for_sleep(display) {
                    log_runtime_line(format!("OLED: sleep prepare failed: {:?}", e).as_str());
                }
            }
            POWER_SAVE_ACTIVE.store(true, Ordering::Relaxed);
            Timer::after(Duration::from_millis(50)).await;
            if let Some(lpwr) = lpwr.take() {
                let mut rtc = Rtc::new(lpwr);
                rtc.sleep_deep(&[&TimerWakeupSource::new(CoreDuration::from_secs(interval_secs))]);
            }
        }

        let elapsed = cycle_started.elapsed();
        let interval = Duration::from_secs(interval_secs);
        wait_watching_button(if elapsed < interval { interval - elapsed } else { Duration::from_millis(0) }).await;
    }
}
