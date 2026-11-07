//! Hardware: DHT22 + battery measurement, radio / ESP-NOW delivery, RTC
//! state and deep sleep. Used by every mode (normal, USB console, portal).

use core::time::Duration as CoreDuration;

use embassy_time::{Duration, Instant, Timer, with_timeout};
use embedded_dht_rs::dht22::Dht22;
use esp_hal::{
    Blocking,
    analog::adc::{Adc, AdcCalCurve, AdcConfig, AdcPin, Attenuation},
    delay::Delay,
    gpio::{DriveMode, Flex, Level, Output, OutputConfig, Pull},
    peripherals::{ADC1, GPIO0, GPIO2, GPIO3, GPIO14, GPIO21, LPWR, USB_DEVICE, WIFI},
    rng::Rng,
    rtc_cntl::{Rtc, sleep::TimerWakeupSource},
};
use esp_radio::esp_now::{BROADCAST_ADDRESS, EspNow, EspNowWifiInterface, PeerInfo};
use esp_radio::wifi::WifiController;

use crate::config::NodeConfig;
use crate::espnow_packet::{self, Kind, MAX_PACKET_LEN, Packet, Reading};
use crate::log_line;

/// DHT22 needs >= 2 s between conversions.
const DHT_RETRY_DELAY_MS: u64 = 2100;
const DHT_ATTEMPTS: u8 = 2;
const UNICAST_ATTEMPTS: u8 = 3;
/// Generous because the gateway answers from the same executor that runs its
/// HTTPS/TLS work, which can delay the ack. Only paid while searching.
const ACK_WAIT_MS: u64 = 150;
const MAX_CHANNEL: u8 = 13;
/// Valid LiPo range; anything else means no divider / floating pin.
const BATTERY_MV_RANGE: core::ops::RangeInclusive<u32> = 2500..=4500;

// ── Sensors ─────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
pub struct Measurement {
    /// Offsets already applied; None if the DHT22 did not answer.
    pub climate: Option<(f32, f32)>,
    /// 0 = unknown / disabled.
    pub battery_mv: u16,
}

pub struct Sensors {
    dht_power: Output<'static>,
    dht_pin: Flex<'static>,
    adc: Adc<'static, ADC1<'static>, Blocking>,
    bat_pin: AdcPin<GPIO0<'static>, ADC1<'static>, AdcCalCurve<ADC1<'static>>>,
}

impl Sensors {
    /// D3/GPIO21 = DHT22 VCC, D2/GPIO2 = DHT22 DATA, D0/GPIO0 = battery divider.
    pub fn new(power: GPIO21<'static>, data: GPIO2<'static>, adc1: ADC1<'static>, bat: GPIO0<'static>) -> Self {
        let mut config = AdcConfig::new();
        let bat_pin = config.enable_pin_with_cal::<_, AdcCalCurve<ADC1<'static>>>(bat, Attenuation::_11dB);
        let adc = Adc::new(adc1, config);
        let mut dht_pin = Flex::new(data);
        // Drive data low while the sensor is unpowered so it is not fed
        // through the line's pull-up.
        dht_pin.apply_output_config(&OutputConfig::default().with_drive_mode(DriveMode::OpenDrain));
        dht_pin.set_output_enable(true);
        dht_pin.set_low();
        Self {
            dht_power: Output::new(power, Level::Low, OutputConfig::default()),
            dht_pin,
            adc,
            bat_pin,
        }
    }

    fn battery_mv(&mut self, cfg: &NodeConfig) -> u16 {
        if !cfg.battery_adc {
            return 0;
        }
        const SAMPLES: u32 = 8;
        let mut sum = 0u32;
        for _ in 0..SAMPLES {
            sum += nb::block!(self.adc.read_oneshot(&mut self.bat_pin)).unwrap_or(0) as u32;
        }
        let battery_mv = ((sum / SAMPLES) as f32 * cfg.battery_divider) as u32;
        if BATTERY_MV_RANGE.contains(&battery_mv) {
            battery_mv as u16
        } else {
            log_line!("Battery: {} mV out of range, reporting unknown", battery_mv);
            0
        }
    }

    async fn read_dht22(&mut self) -> Option<(f32, f32)> {
        for attempt in 1..=DHT_ATTEMPTS {
            if attempt > 1 {
                Timer::after(Duration::from_millis(DHT_RETRY_DELAY_MS)).await;
            }
            // Bit timing is tens of µs: keep the scheduler tick out of the way.
            let pin = &mut self.dht_pin;
            let result = critical_section::with(|_| Dht22::new(&mut *pin, Delay::new()).read());
            match result {
                Ok(r) => return Some((r.temperature, r.humidity)),
                Err(e) => log_line!("DHT22: read failed (attempt {}/{}): {:?}", attempt, DHT_ATTEMPTS, e),
            }
        }
        None
    }

    /// Powers the DHT22, reads battery while it settles, reads, powers off.
    pub async fn measure(&mut self, cfg: &NodeConfig) -> Measurement {
        self.dht_power.set_high();
        let warmup_started = Instant::now();
        let battery_mv = self.battery_mv(cfg);
        let warmup = Duration::from_millis(cfg.dht_warmup_ms as u64);
        let elapsed = warmup_started.elapsed();
        if elapsed < warmup {
            Timer::after(warmup - elapsed).await;
        }

        self.dht_pin.apply_output_config(
            &OutputConfig::default()
                .with_drive_mode(DriveMode::OpenDrain)
                .with_pull(Pull::Up),
        );
        self.dht_pin.set_input_enable(true);
        self.dht_pin.set_high();
        let raw = self.read_dht22().await;
        self.dht_power.set_low();
        self.dht_pin.apply_output_config(&OutputConfig::default().with_drive_mode(DriveMode::OpenDrain));
        self.dht_pin.set_low();

        let climate = raw.map(|(t, h)| {
            (t + cfg.temp_offset, (h + cfg.hum_offset).clamp(0.0, 100.0))
        });
        match climate {
            Some((t, h)) => log_line!("DHT22: {:.1} C {:.1} %, battery {} mV", t, h, battery_mv),
            None => log_line!("DHT22: no reading, battery {} mV", battery_mv),
        }
        Measurement { climate, battery_mv }
    }
}

// ── Radio ───────────────────────────────────────────────────────────────────

/// XIAO on-board RF switch: GPIO3 low enables it, GPIO14 selects the antenna
/// (low = ceramic, high = U.FL). Keep the returned pins alive while the radio runs.
pub fn rf_switch(enable: GPIO3<'static>, select: GPIO14<'static>, external: bool) -> (Output<'static>, Output<'static>) {
    let level = if external { Level::High } else { Level::Low };
    (
        Output::new(enable, Level::Low, OutputConfig::default()),
        Output::new(select, level, OutputConfig::default()),
    )
}

pub struct Radio {
    _controller: WifiController<'static>,
    _rf: (Output<'static>, Output<'static>),
    pub esp_now: EspNow<'static>,
    pub own_mac: [u8; 6],
}

impl Radio {
    /// Starts the radio in (unconnected) station mode for ESP-NOW.
    pub fn start(wifi: WIFI<'static>, rf: (Output<'static>, Output<'static>)) -> Self {
        let (controller, interfaces) =
            esp_radio::wifi::new(wifi, Default::default()).expect("WiFi init failed");
        Self {
            own_mac: interfaces.station.mac_address(),
            esp_now: interfaces.esp_now,
            _controller: controller,
            _rf: rf,
        }
    }
}

// ── State kept in RTC fast memory across deep sleep ─────────────────────────
// `persistent` statics are zeroed on power-on only, not on deep-sleep wake.

const STATE_MAGIC: u32 = 0x434D_4E31; // "CMN1"

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

pub struct NodeState {
    pub boot_id: u32,
    pub seq: u32,
    /// 0 = gateway unknown, search on next send.
    pub channel: u8,
    pub gateway: [u8; 6],
}

impl NodeState {
    pub fn load_or_init() -> Self {
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
        // Cold boot: a fresh boot_id lets the gateway accept the restarted counter.
        Self {
            boot_id: Rng::new().random(),
            seq: 0,
            channel: 0,
            gateway: [0; 6],
        }
    }

    pub fn store(&self) {
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

// ── ESP-NOW delivery ────────────────────────────────────────────────────────

fn add_unicast_peer(esp_now: &EspNow<'static>, mac: &[u8; 6]) -> bool {
    esp_now.peer_exists(mac)
        || esp_now
            .add_peer(PeerInfo {
                interface: EspNowWifiInterface::Station,
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

pub fn build_reading(cfg: &NodeConfig, state: &NodeState, m: &Measurement) -> Reading {
    fn to_centi(value: f32) -> i32 {
        let scaled = value * 100.0;
        (if scaled >= 0.0 { scaled + 0.5 } else { scaled - 0.5 }) as i32
    }
    let mut reading = Reading::empty();
    reading.boot_id = state.boot_id;
    reading.battery_mv = m.battery_mv;
    reading.interval_s = cfg.interval_secs.min(u16::MAX as u32) as u16;
    reading.set_name(cfg.name.as_str());
    match m.climate {
        Some((t, h)) => {
            reading.temp_centi = to_centi(t).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            reading.hum_centi = to_centi(h) as u16;
        }
        None => reading.flags |= espnow_packet::FLAG_SENSOR_ERROR,
    }
    reading
}

/// Sends the reading; updates `state` with the gateway location on success.
pub async fn deliver(radio: &mut Radio, key: &[u8], state: &mut NodeState, reading: &mut Reading) -> bool {
    let esp_now = &mut radio.esp_now;
    let own_mac = radio.own_mac;
    let mut buf = [0u8; MAX_PACKET_LEN];

    // Fast path: unicast to the known gateway; the MAC-layer ACK confirms delivery.
    if state.channel != 0 {
        if esp_now.set_channel(state.channel).is_ok() && add_unicast_peer(esp_now, &state.gateway) {
            reading.seq = state.next_seq();
            let len = espnow_packet::encode_reading(key, &own_mac, Kind::Reading, reading, &mut buf);
            for attempt in 1..=UNICAST_ATTEMPTS {
                match esp_now.send_async(&state.gateway, &buf[..len]).await {
                    Ok(()) => return true,
                    Err(e) => {
                        log_line!("ESP-NOW: unicast attempt {}/{} failed: {:?}", attempt, UNICAST_ATTEMPTS, e);
                        Timer::after(Duration::from_millis(15 * attempt as u64)).await;
                    }
                }
            }
        }
        log_line!("ESP-NOW: gateway not reachable on channel {}, searching...", state.channel);
    }

    // Search: broadcast on each channel (last known first) until the gateway acks.
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
            log_line!(
                "ESP-NOW: gateway {} found on channel {}",
                mac_str(&gateway),
                channel
            );
            state.channel = channel;
            state.gateway = gateway;
            return true;
        }
    }
    // Keep a known gateway/channel as the hint for the next wake: the gateway
    // may just be rebooting, and a unicast try is much cheaper than a search.
    false
}

/// Measure, send, persist state. Returns (measurement, delivered).
pub async fn measure_and_send(
    cfg: &NodeConfig,
    sensors: &mut Sensors,
    radio: &mut Radio,
    state: &mut NodeState,
) -> (Measurement, bool) {
    let measurement = sensors.measure(cfg).await;
    if !cfg.has_key() {
        log_line!("ESP-NOW: no key configured, not sending (set key via USB console or portal)");
        return (measurement, false);
    }
    let mut reading = build_reading(cfg, state, &measurement);
    let delivered = deliver(radio, cfg.key.as_bytes(), state, &mut reading).await;
    state.store();
    log_line!(
        "ESP-NOW: {} (seq {})",
        if delivered { "delivered" } else { "gateway not found" },
        reading.seq
    );
    (measurement, delivered)
}

// ── Misc ────────────────────────────────────────────────────────────────────

pub fn mac_str(mac: &[u8; 6]) -> alloc::string::String {
    alloc::format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    )
}

/// USB SOF frame index; changes every 1 ms while a USB host is attached.
pub fn usb_sof_frame() -> u16 {
    USB_DEVICE::regs().fram_num().read().sof_frame_index().bits()
}

pub fn deep_sleep(lpwr: LPWR<'static>, secs: u64) -> ! {
    let mut rtc = Rtc::new(lpwr);
    rtc.sleep_deep(&[&TimerWakeupSource::new(CoreDuration::from_secs(secs))]);
}
