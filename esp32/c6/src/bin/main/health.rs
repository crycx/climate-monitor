//! Self-supervision: watchdog, DHT22 health and the reason of the last restart.
//!
//! Background (2026-11-24): the board kept its last temperature/humidity in
//! Home Assistant for ~9 h while it was clearly not measuring, and only a
//! manual reset helped. Nothing on the board noticed. This module makes it
//! notice:
//!
//! - **Watchdog.** A supervisor task restarts the board when the main loop has
//!   not completed an iteration for [`MAIN_LOOP_STALL_SECS`] or the ESP-NOW
//!   forward task has been stuck on one reading for [`FORWARD_STALL_SECS`].
//!   The supervisor itself feeds the hardware watchdog (MWDT of TIMG1), so a
//!   hung executor (blocking loop, interrupts off) resets the chip after
//!   [`HW_WDT_TIMEOUT_SECS`]. This is the bare-metal equivalent of the ESP-IDF
//!   task watchdog: there is no FreeRTOS task list here, the main loop and
//!   the tasks are futures on one executor.
//! - **Sensor health.** [`SensorHealth`] counts read errors, detects values
//!   that stopped changing ([`FROZEN_READING_SECS`]) and escalates:
//!   recovery of the DHT22 line first, then a restart.
//! - **Restart reason.** Our own restarts record why in RTC RAM that survives
//!   a software reset; [`reset_reason_text`] reports it to HA together with the
//!   chip's reset reason.

use core::sync::atomic::{AtomicU32, Ordering};

use embassy_time::{Duration, Instant, Timer};
use esp_hal::{
    peripherals::TIMG1,
    rtc_cntl::SocResetReason,
    timer::timg::{MwdtStage, Wdt},
};

// ── Tunables ────────────────────────────────────────────────────────────────

/// Hardware watchdog (MWDT) timeout. Fed by the supervisor task every
/// [`WDT_FEED_MS`], so it only fires when the whole executor stops running
/// (a blocking busy loop or interrupts left off). The longest legitimate
/// blocking sections are a few seconds (TLS handshake maths, keep-alive pulse
/// up to 5 s, flash erases are per sector).
pub const HW_WDT_TIMEOUT_SECS: u64 = 30;
/// How often the supervisor checks the deadlines and feeds the MWDT.
const WDT_FEED_MS: u64 = 1000;
/// The main loop must finish an iteration within this time (it normally
/// loops every 250 ms). The longest legitimate iteration is a lost WiFi
/// connection with failover across 5 networks (~5 min), so 10 min leaves room.
pub const MAIN_LOOP_STALL_SECS: u64 = 600;
/// Deadline right after boot, before the main loop runs (boot WiFi connect
/// with failover).
const BOOT_STALL_SECS: u64 = 600;
/// Extra time the main loop may take while updating its own firmware.
pub const SELF_OTA_ALLOWANCE_SECS: u64 = 900;
/// The ESP-NOW forward task may spend at most this long on one reading
/// (MQTT publish, backend POST, staging a sensor firmware for OTA).
pub const FORWARD_STALL_SECS: u64 = 600;

/// Consecutive bad readings after which temperature/humidity are reported
/// unavailable in HA (a single failed read is only skipped). A frozen value
/// is reported unavailable immediately.
pub const SENSOR_ERRORS_BEFORE_UNAVAILABLE: u32 = 2;
/// N: consecutive bad readings (read error, out of range) before the DHT22
/// line is recovered. At the default 60 s interval: 5 min.
pub const SENSOR_ERRORS_BEFORE_RECOVERY: u32 = 5;
/// Recoveries that may fail before the board restarts. After the recovery,
/// another [`SENSOR_ERRORS_BEFORE_RECOVERY`] bad readings trigger the restart.
pub const SENSOR_RECOVERIES_BEFORE_RESTART: u32 = 1;
/// X: raw temperature AND humidity unchanged for this long count as a frozen
/// sensor (a real room changes humidity by 0.1 % within minutes).
pub const FROZEN_READING_SECS: u64 = 60 * 60;
/// ...and for at least this many readings, so a long interval (up to 1 h)
/// does not call two equal readings frozen.
pub const FROZEN_MIN_SAMPLES: u32 = 5;
/// Sensor-caused restarts in a row (no changing reading in between) before
/// the board stops restarting for the sensor. It then keeps reporting the
/// sensor unavailable and retries the recovery. Protects the ESP-NOW gateway
/// role from a restart loop when the DHT22 is broken or not fitted.
pub const MAX_SENSOR_RESTARTS: u32 = 3;
/// Idle time with the DHT22 data line released during a recovery. The DHT22
/// needs ≥ 1 s after power-up and ≥ 2 s between reads.
pub const DHT_RECOVERY_SETTLE_MS: u64 = 2500;
/// Plausible DHT22 range (datasheet: -40..80 °C, 0..100 %RH), raw values
/// before the calibration offsets.
pub const DHT_TEMP_MIN_C: f32 = -40.0;
pub const DHT_TEMP_MAX_C: f32 = 80.0;
pub const DHT_HUM_MIN_PCT: f32 = 0.0;
pub const DHT_HUM_MAX_PCT: f32 = 100.0;

// ── Restart cause (RTC RAM, survives software reset) ────────────────────────

const RTC_MAGIC: u32 = 0xC6A1_7E57;

/// [magic, cause, sensor restarts in a row, checksum]. Persistent: not
/// initialised on boot, so it is validated with magic + checksum.
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut RTC_STATE: [u32; 4] = [0; 4];

/// Why this firmware restarted the board itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Cause {
    None = 0,
    Panic = 1,
    MainLoopStall = 2,
    ForwardStall = 3,
    SensorErrors = 4,
    SensorFrozen = 5,
    Console = 6,
}

impl Cause {
    fn from_u32(v: u32) -> Self {
        match v {
            1 => Self::Panic,
            2 => Self::MainLoopStall,
            3 => Self::ForwardStall,
            4 => Self::SensorErrors,
            5 => Self::SensorFrozen,
            6 => Self::Console,
            _ => Self::None,
        }
    }

    fn text(self) -> &'static str {
        match self {
            Self::None => "Software restart",
            Self::Panic => "Firmware panic",
            Self::MainLoopStall => "Watchdog: main loop stalled",
            Self::ForwardStall => "Watchdog: ESP-NOW forwarding stalled",
            Self::SensorErrors => "Sensor errors (DHT22)",
            Self::SensorFrozen => "Sensor frozen (DHT22)",
            Self::Console => "Console restart",
        }
    }
}

fn rtc_checksum(s: &[u32; 4]) -> u32 {
    s[0] ^ s[1].rotate_left(8) ^ s[2].rotate_left(16) ^ 0x5A5A_5A5A
}

fn rtc_load() -> (Cause, u32) {
    // SAFETY: single-core; only touched from here and from the panic handler.
    let s = unsafe { core::ptr::read_volatile(&raw const RTC_STATE) };
    if s[0] == RTC_MAGIC && s[3] == rtc_checksum(&s) {
        (Cause::from_u32(s[1]), s[2])
    } else {
        (Cause::None, 0)
    }
}

fn rtc_store(cause: Cause, sensor_restarts: u32) {
    let mut s = [RTC_MAGIC, cause as u32, sensor_restarts, 0];
    s[3] = rtc_checksum(&s);
    // SAFETY: see rtc_load.
    unsafe { core::ptr::write_volatile(&raw mut RTC_STATE, s) };
}

/// Restart cause and sensor-restart streak read at boot (see [`init`]).
static BOOT_CAUSE: AtomicU32 = AtomicU32::new(0);
static BOOT_RESET: AtomicU32 = AtomicU32::new(0);
static SENSOR_RESTARTS: AtomicU32 = AtomicU32::new(0);

/// Total bad readings (errors, out of range, frozen) since boot.
static SENSOR_ERRORS: AtomicU32 = AtomicU32::new(0);

/// Reads (and clears) the recorded restart cause. Call once at boot.
pub fn init() {
    let (cause, sensor_restarts) = rtc_load();
    let reset = esp_hal::rtc_cntl::reset_reason(esp_hal::system::Cpu::ProCpu);
    // A recorded cause only explains a software reset; after a power cycle
    // or a hardware watchdog the RTC contents are stale.
    let software = matches!(reset, Some(SocResetReason::CoreSw) | Some(SocResetReason::Cpu0Sw));
    let cause = if software { cause } else { Cause::None };
    let sensor_restarts = if software { sensor_restarts } else { 0 };
    BOOT_CAUSE.store(cause as u32, Ordering::Relaxed);
    BOOT_RESET.store(reset.map_or(0, |r| r as u32), Ordering::Relaxed);
    SENSOR_RESTARTS.store(sensor_restarts, Ordering::Relaxed);
    // Keep the streak, forget the cause: the next unrecorded software reset
    // (OTA, flashing tool) must not repeat this one.
    rtc_store(Cause::None, sensor_restarts);
    DEADLINES.main.store(now_secs() + BOOT_STALL_SECS as u32, Ordering::Relaxed);
}

/// Restart reason as text for HA ("Power on", "Watchdog: main loop stalled", …).
pub fn reset_reason_text() -> &'static str {
    let cause = Cause::from_u32(BOOT_CAUSE.load(Ordering::Relaxed));
    let reset = SocResetReason::from_repr(BOOT_RESET.load(Ordering::Relaxed) as usize);
    match reset {
        Some(SocResetReason::CoreSw | SocResetReason::Cpu0Sw) => cause.text(),
        Some(SocResetReason::ChipPowerOn) => "Power on",
        Some(SocResetReason::SysBrownOut) => "Brownout",
        Some(SocResetReason::CoreDeepSleep) => "Deep sleep wake",
        Some(
            SocResetReason::CoreMwdt0
            | SocResetReason::CoreMwdt1
            | SocResetReason::Cpu0Mwdt0
            | SocResetReason::Cpu0Mwdt1,
        ) => "Watchdog: system hung (MWDT)",
        Some(SocResetReason::CoreRtcWdt | SocResetReason::Cpu0RtcWdt | SocResetReason::SysRtcWdt) => {
            "Watchdog: RTC WDT"
        }
        Some(SocResetReason::SysSuperWdt) => "Watchdog: super WDT",
        Some(SocResetReason::CoreUsbUart | SocResetReason::CoreUsbJtag | SocResetReason::Cpu0JtagCpu) => {
            "USB / flashing tool"
        }
        Some(SocResetReason::CoreEfuseCrc) => "eFuse CRC error",
        Some(SocResetReason::CoreSDIO) => "SDIO",
        None => "Unknown",
    }
}

/// Seconds since boot.
pub fn uptime_secs() -> u64 {
    Instant::now().as_secs()
}

fn now_secs() -> u32 {
    Instant::now().as_secs() as u32
}

/// Bad DHT22 readings since boot.
pub fn sensor_errors() -> u32 {
    SENSOR_ERRORS.load(Ordering::Relaxed)
}

/// Records `cause` and restarts the board.
pub fn restart(cause: Cause) -> ! {
    let streak = SENSOR_RESTARTS.load(Ordering::Relaxed);
    let streak = match cause {
        Cause::SensorErrors | Cause::SensorFrozen => streak.saturating_add(1),
        _ => streak,
    };
    rtc_store(cause, streak);
    crate::log_runtime_line(alloc::format!("Restart: {}", cause.text()).as_str());
    // Let the USB console drain.
    esp_hal::delay::Delay::new().delay_millis(200);
    esp_hal::system::software_reset()
}

/// For the panic handler: record the cause without touching the allocator.
pub fn record_panic() {
    let streak = SENSOR_RESTARTS.load(Ordering::Relaxed);
    rtc_store(Cause::Panic, streak);
}

// ── Watchdog ────────────────────────────────────────────────────────────────

struct Deadlines {
    /// Uptime second by which the main loop must check in again.
    main: AtomicU32,
    /// Uptime second the forward task started its current reading (0 = idle).
    forward_since: AtomicU32,
}

static DEADLINES: Deadlines = Deadlines {
    main: AtomicU32::new(u32::MAX),
    forward_since: AtomicU32::new(0),
};

/// Called by the main loop on every iteration.
pub fn main_loop_alive() {
    DEADLINES.main.store(now_secs() + MAIN_LOOP_STALL_SECS as u32, Ordering::Relaxed);
}

/// Before a known long step of the main loop (power-bank wait, own OTA):
/// allow `secs` (plus the normal margin) until the next check-in.
pub fn main_loop_allow(secs: u64) {
    let secs = secs.saturating_add(MAIN_LOOP_STALL_SECS).min(u32::MAX as u64 / 2) as u32;
    DEADLINES.main.store(now_secs().saturating_add(secs), Ordering::Relaxed);
}

/// The forward task starts / finishes handling one reading.
pub fn forward_begin() {
    DEADLINES.forward_since.store(now_secs().max(1), Ordering::Relaxed);
}

pub fn forward_end() {
    DEADLINES.forward_since.store(0, Ordering::Relaxed);
}

/// Enables the hardware watchdog and spawns the supervisor that feeds it.
pub fn start_watchdog(spawner: &embassy_executor::Spawner, mut wdt: Wdt<TIMG1<'static>>) {
    wdt.set_timeout(
        MwdtStage::Stage0,
        esp_hal::time::Duration::from_secs(HW_WDT_TIMEOUT_SECS),
    );
    // enable() also sets stage 0 to "reset system".
    wdt.enable();
    wdt.feed();
    spawner.spawn(watchdog_task(wdt).unwrap());
}

#[embassy_executor::task]
async fn watchdog_task(mut wdt: Wdt<TIMG1<'static>>) {
    loop {
        let now = now_secs();
        let main_deadline = DEADLINES.main.load(Ordering::Relaxed);
        if now > main_deadline {
            restart(Cause::MainLoopStall);
        }
        let since = DEADLINES.forward_since.load(Ordering::Relaxed);
        if since != 0 && now.saturating_sub(since) > FORWARD_STALL_SECS as u32 {
            restart(Cause::ForwardStall);
        }
        wdt.feed();
        Timer::after(Duration::from_millis(WDT_FEED_MS)).await;
    }
}

// ── DHT22 health ────────────────────────────────────────────────────────────

/// What the main loop should do after a bad reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SensorAction {
    /// Keep going; the reading is just not reported.
    None,
    /// Recover the DHT22 line (release it, re-apply the pin config, settle).
    Recover,
    /// Restart the board with this cause.
    Restart(Cause),
}

/// Why a reading was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadReading {
    /// Read failed (timeout, checksum, pin).
    Error,
    /// Read succeeded but the value is impossible (out of range, NaN, all-zero frame).
    Implausible,
    /// Temperature and humidity have not changed for [`FROZEN_READING_SECS`].
    Frozen,
}

pub struct SensorHealth {
    /// Bad readings since the escalation step last acted.
    consecutive_bad: u32,
    /// Bad readings since the last good one.
    bad_streak: u32,
    recoveries: u32,
    /// A plausible reading was seen since boot (the sensor is fitted).
    seen_valid: bool,
    /// Last raw reading in tenths, and since when / how often it repeated.
    last_raw: Option<(i32, i32)>,
    same_since: Instant,
    same_count: u32,
}

impl SensorHealth {
    pub const fn new() -> Self {
        Self {
            consecutive_bad: 0,
            bad_streak: 0,
            recoveries: 0,
            seen_valid: false,
            last_raw: None,
            same_since: Instant::from_ticks(0),
            same_count: 0,
        }
    }

    /// Checks a successful read (raw values, before offsets).
    pub fn check(&mut self, temp_c: f32, hum_pct: f32) -> Result<(), BadReading> {
        let plausible = temp_c.is_finite()
            && hum_pct.is_finite()
            && (DHT_TEMP_MIN_C..=DHT_TEMP_MAX_C).contains(&temp_c)
            && (DHT_HUM_MIN_PCT..=DHT_HUM_MAX_PCT).contains(&hum_pct)
            // An all-zero frame passes the checksum (0+0+0+0 == 0).
            && !(temp_c == 0.0 && hum_pct == 0.0);
        if !plausible {
            return Err(BadReading::Implausible);
        }
        self.seen_valid = true;

        let raw = ((temp_c * 10.0) as i32, (hum_pct * 10.0) as i32);
        let now = Instant::now();
        if self.last_raw == Some(raw) {
            self.same_count = self.same_count.saturating_add(1);
        } else {
            // A change against an earlier reading of this boot proves the
            // sensor is alive: a later problem may restart the board again.
            // (The first reading after boot proves nothing: a frozen sensor
            // returns its value once more.)
            if self.last_raw.is_some() && SENSOR_RESTARTS.swap(0, Ordering::Relaxed) != 0 {
                rtc_store(Cause::None, 0);
            }
            self.last_raw = Some(raw);
            self.same_since = now;
            self.same_count = 1;
        }
        let frozen = self.same_count >= FROZEN_MIN_SAMPLES
            && now.duration_since(self.same_since) >= Duration::from_secs(FROZEN_READING_SECS);
        if frozen {
            return Err(BadReading::Frozen);
        }
        self.consecutive_bad = 0;
        self.bad_streak = 0;
        self.recoveries = 0;
        Ok(())
    }

    /// Bad readings since the last good one.
    pub fn bad_streak(&self) -> u32 {
        self.bad_streak
    }

    /// Records a bad reading and decides how to escalate.
    pub fn on_bad(&mut self, why: BadReading) -> SensorAction {
        SENSOR_ERRORS.fetch_add(1, Ordering::Relaxed);
        self.bad_streak = self.bad_streak.saturating_add(1);
        self.consecutive_bad = self.consecutive_bad.saturating_add(1);
        // A frozen value was already unchanged for an hour: escalate now.
        let frozen = why == BadReading::Frozen;
        if !frozen && self.consecutive_bad < SENSOR_ERRORS_BEFORE_RECOVERY {
            return SensorAction::None;
        }
        self.consecutive_bad = 0;
        if self.recoveries < SENSOR_RECOVERIES_BEFORE_RESTART {
            self.recoveries += 1;
            return SensorAction::Recover;
        }
        // Restart only for a sensor that worked in this boot (or froze):
        // without a DHT22 fitted, or one that is dead since boot, a restart
        // does not help and would only interrupt the ESP-NOW gateway.
        let may_restart = (self.seen_valid || frozen)
            && SENSOR_RESTARTS.load(Ordering::Relaxed) < MAX_SENSOR_RESTARTS;
        if may_restart {
            SensorAction::Restart(if frozen { Cause::SensorFrozen } else { Cause::SensorErrors })
        } else {
            SensorAction::Recover
        }
    }
}
