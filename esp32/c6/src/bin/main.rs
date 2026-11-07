#![no_std]
#![no_main]
#![recursion_limit = "256"]
#![allow(unexpected_cfgs)]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

/// Panic handler: prindib vea consolesse.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    esp_println::println!("\n\n*** PANIC ***\n{}", info);
    loop {}
}

#[cfg(feature = "wifi")]
extern crate alloc;

#[cfg(feature = "wifi")]
use alloc::{format, string::String, vec::Vec};
#[cfg(feature = "wifi")]
use core::time::Duration as CoreDuration;
#[cfg(feature = "wifi")]
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

use embedded_dht_rs::dht22::Dht22;
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    delay::Delay,
    gpio::{DriveMode, Flex, OutputConfig, Pull},
    i2c::master::{Config as I2cConfig, I2c},
    rmt::{PulseCode, Tx, TxChannelConfig, TxChannelCreator},
    time::Rate,
};
#[cfg(not(feature = "wifi"))]
use esp_hal::time::Instant as HalInstant;
use ssd1306::{I2CDisplayInterface, Ssd1306, prelude::*};
#[cfg(feature = "wifi")]
use esp_hal::gpio::{Input, InputConfig, Level};

// ── WiFi-only imports ────────────────────────────────────────────────────────
#[cfg(feature = "wifi")]
use embassy_executor::Spawner;
#[cfg(feature = "wifi")]
use embassy_net::{Stack, dns::DnsSocket, tcp::client::TcpClientState};
#[cfg(feature = "wifi")]
use embassy_net::{IpAddress, IpEndpoint, Ipv4Address};
#[cfg(feature = "wifi")]
use embassy_net::dns::DnsQueryType;
#[cfg(feature = "wifi")]
use embassy_net::tcp::TcpSocket;
#[cfg(feature = "wifi")]
use embassy_time::{Duration, Instant, Timer};
#[cfg(feature = "wifi")]
use esp_hal::interrupt::software::SoftwareInterruptControl;
#[cfg(feature = "wifi")]
use esp_hal::rtc_cntl::{Rtc, sleep::TimerWakeupSource};
#[cfg(feature = "wifi")]
use esp_hal::timer::timg::TimerGroup;
#[cfg(feature = "wifi")]
use esp_storage::FlashStorage;
#[cfg(feature = "wifi")]
use reqwless::{
    client::HttpClient,
    request::{Method, RequestBuilder},
    headers::ContentType,
};
#[cfg(all(feature = "wifi", not(feature = "no-tls")))]
use reqwless::client::{TlsConfig, TlsVerify};
#[cfg(feature = "wifi")]
use alloc::boxed::Box;
#[cfg(feature = "wifi")]
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
#[cfg(feature = "wifi")]
use static_cell::StaticCell;

#[cfg(feature = "wifi")]
use core::net::Ipv4Addr;

#[cfg(feature = "wifi")]
#[path = "main/app_config.rs"]
mod app_config;
#[cfg(feature = "wifi")]
#[path = "main/config_portal.rs"]
mod config_portal;
#[cfg(feature = "wifi")]
#[path = "main/wifi_runtime.rs"]
mod wifi_runtime;
#[cfg(feature = "wifi")]
#[path = "../../../shared/espnow_packet.rs"]
mod espnow_packet;
#[cfg(feature = "wifi")]
#[path = "main/espnow_gateway.rs"]
mod espnow_gateway;
#[cfg(feature = "wifi")]
#[path = "main/usb_console.rs"]
mod usb_console;

#[cfg(feature = "wifi")]
use app_config::{AppConfig, FlashConfigStorage, resolve_partition_range};
#[cfg(feature = "wifi")]
use wifi_runtime::WifiCommand;

#[path = "../oled_display.rs"]
mod oled_display;

// This creates a default app-descriptor required by the ESP-IDF bootloader.
esp_bootloader_esp_idf::esp_app_desc!();

// ── Static storage for embassy-net (must be 'static) ────────────────────────
#[cfg(feature = "wifi")]
static NET_STACK: StaticCell<Stack<'static>> = StaticCell::new();
#[cfg(feature = "wifi")]
static TCP_STATE: StaticCell<embassy_net::tcp::client::TcpClientState<2, 4096, 4096>> = StaticCell::new();
#[cfg(feature = "wifi")]
static BUTTON_PRESSED: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "wifi")]
static AP_ACTIVE: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "wifi")]
static SENDING: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "wifi")]
static BOOTING: AtomicBool = AtomicBool::new(true);
#[cfg(feature = "wifi")]
static POWER_SAVE_ACTIVE: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "wifi")]
static WIFI_CONNECTING: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "wifi")]
static LAST_SENT_SECS: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "wifi")]
static BUTTON_EVENT: AtomicU8 = AtomicU8::new(BUTTON_EVENT_NONE);
/// Saved WiFi slot (1..=5) currently in use; selects the per-slot MQTT toggle.
#[cfg(feature = "wifi")]
static ACTIVE_WIFI_SLOT: AtomicUsize = AtomicUsize::new(1);

// Retained across deep-sleep resets: remaining seconds until the next full measurement cycle.
#[cfg(feature = "wifi")]
#[esp_hal::ram(unstable(rtc_fast))]
static mut DEEP_SLEEP_KEEP_ALIVE_REMAINING_SECS: u32 = 0;

#[cfg(feature = "wifi")]
const AP_SSID_PREFIX: &str = "ClimateMonitor-";
#[cfg(feature = "wifi")]
const DEFAULT_SENSOR_INTERVAL_SECS: u64 = 30;
#[cfg(feature = "wifi")]
const MIN_SENSOR_INTERVAL_SECS: u64 = 5;
#[cfg(feature = "wifi")]
const MAX_SENSOR_INTERVAL_SECS: u64 = 3600;
#[cfg(feature = "wifi")]
const MAX_DEEP_SLEEP_KEEP_ALIVE_PULSE_MS: u64 = 5000;
#[cfg(feature = "wifi")]
const MAX_DEEP_SLEEP_KEEP_ALIVE_WAKE_INTERVAL_SECS: u64 = 3600;
#[cfg(feature = "wifi")]
const MAIN_LOOP_TICK_MS: u64 = 250;
#[cfg(feature = "wifi")]
const OLED_REFRESH_MS: u64 = 1000;
/// OLED page rotation between own reading and ESP-NOW sensors.
#[cfg(feature = "wifi")]
const OLED_PAGE_MS: u64 = 5000;
/// How long a freshly received ESP-NOW reading stays on screen.
#[cfg(feature = "wifi")]
const OLED_REMOTE_HOLD_MS: u64 = 10_000;
#[cfg(feature = "wifi")]
const PRECONNECT_SCAN_WAIT_MS: u64 = 2500;
#[cfg(feature = "wifi")]
const BOOT_PRESCAN_ATTEMPTS: u8 = 4;
const OLED_BOOT_INIT_RETRIES: u8 = 5;
const OLED_BOOT_INIT_RETRY_DELAY_MS: u32 = 150;
#[cfg(feature = "wifi")]
const SAVE_AP_CLOSE_DELAY_SECS: u64 = 2;
#[cfg(feature = "wifi")]
const BUTTON_EVENT_NONE: u8 = 0;
#[cfg(feature = "wifi")]
const BUTTON_EVENT_SHORT: u8 = 1;
#[cfg(feature = "wifi")]
const BUTTON_EVENT_LONG: u8 = 2;
#[cfg(feature = "wifi")]
const LED_SENT_GREEN_WINDOW_SECS: usize = 2;

fn oled_i2c_address() -> u8 {
    match option_env!("OLED_I2C_ADDR") {
        Some("0x3D") | Some("0X3D") | Some("61") => 0x3D,
        _ => 0x3C,
    }
}

#[cfg(feature = "wifi")]
fn oled_enabled() -> bool {
    match option_env!("OLED_ENABLED") {
        Some("0" | "false" | "FALSE" | "no" | "NO" | "off" | "OFF") => false,
        Some("1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON") => true,
        // Super Mini boards are often used without SSD1306 attached.
        None => !cfg!(feature = "supermini"),
        _ => !cfg!(feature = "supermini"),
    }
}

#[cfg(feature = "wifi")]
fn oled_boot_scan_enabled() -> bool {
    matches!(
        option_env!("OLED_BOOT_SCAN"),
        Some("1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON")
    )
}

#[cfg(feature = "wifi")]
fn log_runtime_line(line: &str) {
    esp_println::println!("{}", line);
    wifi_runtime::portal_log(line);
}

#[cfg(feature = "wifi")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ButtonEvent {
    ShortPress,
    LongPress,
}

#[cfg(feature = "wifi")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PortalMode {
    /// AP is down, normal sensor-post mode.
    Off,
    /// AP opened because no WiFi credentials are configured.
    MissingConfig,
    /// User manually opened the portal via button.
    Manual,
    /// AP reopened automatically because STA reconnect failed.
    Recovery,
}

#[cfg(feature = "wifi")]
impl PortalMode {
    fn is_open(self) -> bool {
        !matches!(self, Self::Off)
    }
}

#[cfg(feature = "wifi")]
fn take_button_event() -> Option<ButtonEvent> {
    let raw = BUTTON_EVENT.load(Ordering::Relaxed);
    if raw == BUTTON_EVENT_NONE {
        return None;
    }

    BUTTON_EVENT.store(BUTTON_EVENT_NONE, Ordering::Relaxed);
    match raw {
        BUTTON_EVENT_SHORT => Some(ButtonEvent::ShortPress),
        BUTTON_EVENT_LONG => Some(ButtonEvent::LongPress),
        _ => None,
    }
}

#[cfg(feature = "wifi")]
fn long_press_pending() -> bool {
    BUTTON_EVENT.load(Ordering::Relaxed) == BUTTON_EVENT_LONG
}

#[cfg(feature = "wifi")]
async fn wait_for_scan_settle_interruptible(total: Duration) -> bool {
    let mut waited = Duration::from_millis(0);
    let step = Duration::from_millis(100);

    while waited < total {
        if long_press_pending() {
            log_runtime_line("WiFi pre-scan aborted: long BOOT press detected (force AP pending).");
            return false;
        }

        let remaining = if total > waited {
            total - waited
        } else {
            Duration::from_millis(0)
        };
        let chunk = if remaining > step { step } else { remaining };
        Timer::after(chunk).await;
        waited += chunk;
    }

    true
}

#[cfg(feature = "wifi")]
fn has_wifi_config(cfg: &AppConfig) -> bool {
    !cfg.wifi_ssid.is_empty()
        || !cfg.wifi_ssid_2.is_empty()
        || !cfg.wifi_ssid_3.is_empty()
        || !cfg.wifi_ssid_4.is_empty()
        || !cfg.wifi_ssid_5.is_empty()
}

#[cfg(feature = "wifi")]
fn sensor_interval_secs(cfg: &AppConfig) -> u64 {
    cfg
        .sensor_interval_secs
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|secs| (MIN_SENSOR_INTERVAL_SECS..=MAX_SENSOR_INTERVAL_SECS).contains(secs))
        .unwrap_or(DEFAULT_SENSOR_INTERVAL_SECS)
}

#[cfg(feature = "wifi")]
fn deep_sleep_keep_alive_pulse_ms(cfg: &AppConfig) -> u64 {
    cfg.deep_sleep_keep_alive_pulse_ms
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|ms| *ms <= MAX_DEEP_SLEEP_KEEP_ALIVE_PULSE_MS)
        .unwrap_or(0)
}

#[cfg(feature = "wifi")]
fn deep_sleep_keep_alive_wake_interval_secs(cfg: &AppConfig) -> u64 {
    cfg.deep_sleep_keep_alive_wake_interval_secs
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|secs| *secs <= MAX_DEEP_SLEEP_KEEP_ALIVE_WAKE_INTERVAL_SECS)
        .unwrap_or(0)
}

#[cfg(feature = "wifi")]
fn is_power_bank_mode(cfg: &AppConfig) -> bool {
    cfg.power_source.trim() == "power_bank"
}

#[cfg(feature = "wifi")]
fn oled_wait_mode(cfg: &AppConfig) -> &str {
    match cfg.oled_wait_mode.trim() {
        "off" => "off",
        "on" => "on",
        _ => "dim",
    }
}

#[cfg(feature = "wifi")]
fn clear_keep_alive_sleep_plan() {
    unsafe {
        DEEP_SLEEP_KEEP_ALIVE_REMAINING_SECS = 0;
    }
}

#[cfg(feature = "wifi")]
fn has_keep_alive_sleep_plan() -> bool {
    unsafe { DEEP_SLEEP_KEEP_ALIVE_REMAINING_SECS > 0 }
}

#[cfg(feature = "wifi")]
fn next_keep_alive_sleep_chunk_secs(total_interval_secs: u64, wake_interval_secs: u64) -> (u64, bool) {
    if wake_interval_secs == 0 || wake_interval_secs >= total_interval_secs {
        clear_keep_alive_sleep_plan();
        return (total_interval_secs, false);
    }

    unsafe {
        let mut remaining_secs = if DEEP_SLEEP_KEEP_ALIVE_REMAINING_SECS == 0 {
            total_interval_secs
        } else {
            DEEP_SLEEP_KEEP_ALIVE_REMAINING_SECS as u64
        };

        let chunk_secs = remaining_secs.min(wake_interval_secs);
        remaining_secs = remaining_secs.saturating_sub(chunk_secs);
        DEEP_SLEEP_KEEP_ALIVE_REMAINING_SECS = remaining_secs as u32;

        (chunk_secs, remaining_secs > 0)
    }
}

#[cfg(feature = "wifi")]
fn wifi_display_label(location: &str, ssid: &str) -> String {
    if location.is_empty() {
        ssid.into()
    } else {
        format!("{}/{}", location, ssid)
    }
}

#[cfg(feature = "wifi")]
fn oled_rotation_from_config(orientation: &str) -> DisplayRotation {
    match orientation {
        "portrait" => DisplayRotation::Rotate90,
        _ => DisplayRotation::Rotate0,
    }
}

#[cfg(feature = "wifi")]
fn oled_is_portrait(orientation: &str) -> bool {
    orientation == "portrait"
}

#[cfg(feature = "wifi")]
fn stack_has_ipv4(stack: &Stack<'static>) -> bool {
    stack.is_config_up() || stack.config_v4().is_some()
}

#[cfg(feature = "wifi")]
fn wifi_sta_connected() -> bool {
    wifi_runtime::sta_connected()
}

#[cfg(feature = "wifi")]
async fn wait_for_sta_network(stack: &'static Stack<'static>, timeout: Duration) -> bool {
    WIFI_CONNECTING.store(true, Ordering::Relaxed);
    let started = Instant::now();
    while started.elapsed() < timeout {
        if stack_has_ipv4(stack) {
            WIFI_CONNECTING.store(false, Ordering::Relaxed);
            return true;
        }
        Timer::after(Duration::from_millis(250)).await;
    }
    let ok = stack_has_ipv4(stack);
    WIFI_CONNECTING.store(false, Ordering::Relaxed);
    ok
}

#[cfg(feature = "wifi")]
fn draw_status_for_mode<DI>(
    display: &mut oled_display::OledDisplay<DI>,
    status: &str,
    oled_portrait_mode: bool,
) where
    DI: display_interface::WriteOnlyDataCommand,
{
    if oled_portrait_mode {
        if let Err(e) = display.set_rotation(DisplayRotation::Rotate0) {
            log_runtime_line(format!("OLED: set landscape rotation failed: {:?}", e).as_str());
        }

        if let Err(e) = oled_display::draw_status(display, status) {
            log_runtime_line(format!("OLED: draw_status failed: {:?}", e).as_str());
        }

        if let Err(e) = display.set_rotation(DisplayRotation::Rotate90) {
            log_runtime_line(format!("OLED: restore portrait rotation failed: {:?}", e).as_str());
        }
    } else if let Err(e) = oled_display::draw_status(display, status) {
        log_runtime_line(format!("OLED: draw_status failed: {:?}", e).as_str());
    }
}

#[cfg(feature = "wifi")]
async fn set_portal_mode(
    wifi_sender: &wifi_runtime::WifiCommandSender,
    portal_mode: &mut PortalMode,
    next_mode: PortalMode,
    log_line: &str,
) {
    if *portal_mode == next_mode {
        return;
    }

    let was_open = portal_mode.is_open();
    let will_open = next_mode.is_open();

    log_runtime_line(log_line);

    match (was_open, will_open) {
        (false, true) => {
            // Manual portal should be AP-only so configured AP channel (e.g. 6) is applied.
            if next_mode == PortalMode::Manual {
                wifi_sender.send(WifiCommand::StaDown).await;
            }
            wifi_sender
                .send(WifiCommand::APUp(AP_SSID_PREFIX.into()))
                .await;
        }
        (true, false) => {
            wifi_sender.send(WifiCommand::APDown).await;
        }
        _ => {}
    }

    *portal_mode = next_mode;
    AP_ACTIVE.store(will_open, Ordering::Relaxed);
}

#[cfg(feature = "wifi")]
async fn reconnect_after_portal_close(
    wifi_sender: &wifi_runtime::WifiCommandSender,
    stack: &'static Stack<'static>,
    cfg: &AppConfig,
    active_wifi_ssid: &str,
    reason_prefix: &str,
) -> Option<(usize, String, String)> {
    let mut selected = scan_and_connect_best_configured_wifi(
        wifi_sender,
        cfg,
        format!("{} scanning visible WiFi networks before reconnect...", reason_prefix).as_str(),
        format!("{} reconnecting to ", reason_prefix).as_str(),
    )
    .await;

    if selected.is_none() {
        selected = connect_configured_wifi_with_failover(
            wifi_sender,
            stack,
            cfg,
            active_wifi_ssid,
            format!("{} scan reconnect fallback...", reason_prefix).as_str(),
            format!("{} trying ", reason_prefix).as_str(),
            Duration::from_secs(20),
        )
        .await;
    }

    if let Some((slot, ssid, location)) = selected {
        let mut connected = wait_for_sta_network(stack, Duration::from_secs(20)).await;
        if !connected && wifi_sta_connected() {
            log_runtime_line("STA link is up; waiting a bit longer for IPv4...");
            connected = wait_for_sta_network(stack, Duration::from_secs(8)).await;
        }

        if connected {
            log_runtime_line(format!("{} reconnect successful.", reason_prefix).as_str());
            return Some((slot, ssid, location));
        }

        log_runtime_line(format!("{} reconnect timed out after STA connect attempt.", reason_prefix).as_str());
    }

    None
}

#[cfg(feature = "wifi")]
async fn connect_best_configured_wifi(
    wifi_sender: &wifi_runtime::WifiCommandSender,
    cfg: &AppConfig,
    context: &str,
) -> Option<(usize, String, String)> {
    if let Some((slot, ssid, pass, location)) = find_best_ssid(cfg) {
        let selected_ssid = ssid.clone();
        if pass.trim().is_empty() {
            log_runtime_line(
                format!(
                    "WiFi slot {} selected (SSID '{}') with empty password",
                    slot, ssid
                )
                .as_str(),
            );
        } else {
            log_runtime_line(
                format!("WiFi slot {} selected (SSID '{}')", slot, ssid).as_str(),
            );
        }
        if wifi_runtime::scan_ssid_visible(&ssid) {
            log_runtime_line(format!("WiFi pre-scan: visible configured SSID found: {}", ssid).as_str());
        } else if wifi_runtime::has_scan_results() {
            log_runtime_line(
                format!(
                    "WiFi pre-scan: no configured visible SSID matched, using fallback: {}",
                    ssid
                )
                .as_str(),
            );
        } else {
            log_runtime_line(
                format!("WiFi pre-scan: no scan results available, using saved network: {}", ssid)
                    .as_str(),
            );
        }
        log_runtime_line(format!("{}{}", context, ssid).as_str());
        wifi_sender.send(WifiCommand::StaUp(ssid, pass)).await;
        Some((slot, selected_ssid, location))
    } else {
        None
    }
}

#[cfg(feature = "wifi")]
async fn refresh_visible_networks(
    wifi_sender: &wifi_runtime::WifiCommandSender,
    reason: &str,
) {
    let _ = refresh_visible_networks_for(wifi_sender, reason, &[]).await;
}

#[cfg(feature = "wifi")]
async fn refresh_visible_networks_for(
    wifi_sender: &wifi_runtime::WifiCommandSender,
    reason: &str,
    wanted_ssids: &[&str],
) -> bool {
    WIFI_CONNECTING.store(true, Ordering::Relaxed);
    log_runtime_line(reason);
    let mut merged_ssids: Vec<String> = Vec::new();

    for attempt in 1..=BOOT_PRESCAN_ATTEMPTS {
        if long_press_pending() {
            log_runtime_line("WiFi pre-scan cancelled before next attempt due to long BOOT press.");
            WIFI_CONNECTING.store(false, Ordering::Relaxed);
            return false;
        }

        log_runtime_line(format!("WiFi pre-scan attempt {}/{}...", attempt, BOOT_PRESCAN_ATTEMPTS).as_str());
        wifi_sender.send(WifiCommand::ScanOnce).await;
        if !wait_for_scan_settle_interruptible(Duration::from_millis(PRECONNECT_SCAN_WAIT_MS)).await {
            WIFI_CONNECTING.store(false, Ordering::Relaxed);
            return false;
        }

        if let Some(flat) = wifi_runtime::scan_results_flat() {
            for ssid in flat.lines().filter(|ssid| !ssid.is_empty()) {
                if !merged_ssids.iter().any(|existing| existing.as_str() == ssid) {
                    merged_ssids.push(String::from(ssid));
                }
            }
        }

        // Short-circuit: if any wanted SSID is already visible, no need for more scans.
        if !wanted_ssids.is_empty()
            && wanted_ssids
                .iter()
                .any(|w| merged_ssids.iter().any(|s| s.as_str() == *w))
        {
            log_runtime_line(
                format!(
                    "WiFi pre-scan: target SSID found after {} attempt(s), stopping early.",
                    attempt
                )
                .as_str(),
            );
            break;
        }
    }

    if merged_ssids.is_empty() {
        log_runtime_line("WiFi pre-scan complete: no visible networks found.");
        WIFI_CONNECTING.store(false, Ordering::Relaxed);
        return true;
    }

    let mut merged_flat = String::new();
    for ssid in merged_ssids.iter() {
        if !merged_flat.is_empty() {
            merged_flat.push('\n');
        }
        merged_flat.push_str(ssid.as_str());
    }

    wifi_runtime::set_scan_results_from_flat(merged_flat.as_str());
    let count = merged_ssids.len();
    log_runtime_line(
        format!("WiFi pre-scan complete: {} visible network(s) across {} attempts.", count, BOOT_PRESCAN_ATTEMPTS).as_str(),
    );
    log_runtime_line(format!("Visible SSIDs:\n{}", merged_flat).as_str());
    WIFI_CONNECTING.store(false, Ordering::Relaxed);
    true
}

#[cfg(feature = "wifi")]
async fn scan_and_connect_best_configured_wifi(
    wifi_sender: &wifi_runtime::WifiCommandSender,
    cfg: &AppConfig,
    scan_reason: &str,
    connect_context: &str,
) -> Option<(usize, String, String)> {
    // Collect all configured SSIDs so the scan can stop early when one is found.
    let wanted: Vec<&str> = [
        cfg.wifi_ssid.as_str(),
        cfg.wifi_ssid_2.as_str(),
        cfg.wifi_ssid_3.as_str(),
        cfg.wifi_ssid_4.as_str(),
        cfg.wifi_ssid_5.as_str(),
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect();

    if !refresh_visible_networks_for(wifi_sender, scan_reason, &wanted).await {
        return None;
    }
    connect_best_configured_wifi(wifi_sender, cfg, connect_context).await
}

#[cfg(feature = "wifi")]
fn configured_wifi_slots(cfg: &AppConfig) -> [(usize, &str, &str, &str); 5] {
    [
        (1usize, cfg.wifi_ssid.as_str(), cfg.wifi_password.as_str(), cfg.wifi_location.as_str()),
        (
            2usize,
            cfg.wifi_ssid_2.as_str(),
            cfg.wifi_password_2.as_str(),
            cfg.wifi_location_2.as_str(),
        ),
        (
            3usize,
            cfg.wifi_ssid_3.as_str(),
            cfg.wifi_password_3.as_str(),
            cfg.wifi_location_3.as_str(),
        ),
        (
            4usize,
            cfg.wifi_ssid_4.as_str(),
            cfg.wifi_password_4.as_str(),
            cfg.wifi_location_4.as_str(),
        ),
        (
            5usize,
            cfg.wifi_ssid_5.as_str(),
            cfg.wifi_password_5.as_str(),
            cfg.wifi_location_5.as_str(),
        ),
    ]
}

#[cfg(feature = "wifi")]
fn build_wifi_candidates(cfg: &AppConfig, skip_ssid: &str) -> Vec<(usize, String, String, String)> {
    let mut visible: Vec<(usize, String, String, String)> = Vec::new();
    let mut other: Vec<(usize, String, String, String)> = Vec::new();

    for (slot, ssid, pass, location) in configured_wifi_slots(cfg) {
        if ssid.is_empty() || ssid == skip_ssid {
            continue;
        }

        let entry = (
            slot,
            String::from(ssid),
            String::from(pass),
            String::from(location.trim()),
        );

        if wifi_runtime::scan_ssid_visible(ssid) {
            visible.push(entry);
        } else {
            other.push(entry);
        }
    }

    visible.extend(other);
    visible
}

#[cfg(feature = "wifi")]
fn build_visible_wifi_candidates(
    cfg: &AppConfig,
    skip_ssid: &str,
) -> Vec<(usize, String, String, String)> {
    let mut visible: Vec<(usize, String, String, String)> = Vec::new();

    for (slot, ssid, pass, location) in configured_wifi_slots(cfg) {
        if ssid.is_empty() || ssid == skip_ssid {
            continue;
        }
        if wifi_runtime::scan_ssid_visible(ssid) {
            visible.push((
                slot,
                String::from(ssid),
                String::from(pass),
                String::from(location.trim()),
            ));
        }
    }

    visible
}

#[cfg(feature = "wifi")]
async fn connect_configured_wifi_with_failover(
    wifi_sender: &wifi_runtime::WifiCommandSender,
    stack: &'static Stack<'static>,
    cfg: &AppConfig,
    skip_ssid: &str,
    scan_reason: &str,
    connect_context: &str,
    attempt_timeout: Duration,
) -> Option<(usize, String, String)> {
    refresh_visible_networks(wifi_sender, scan_reason).await;

    let has_scan = wifi_runtime::has_scan_results();
    let mut candidates = if has_scan {
        build_visible_wifi_candidates(cfg, skip_ssid)
    } else {
        build_wifi_candidates(cfg, skip_ssid)
    };

    let mut retry_rounds = 1usize;

    if has_scan && candidates.is_empty() {
        log_runtime_line(
            "No configured SSID visible in scan; trying all saved WiFi networks for 3 rounds before AP fallback.",
        );
        candidates = build_wifi_candidates(cfg, "");
        retry_rounds = 3;
    }

    if candidates.is_empty() {
        candidates = build_wifi_candidates(cfg, "");
    }

    if candidates.is_empty() {
        return None;
    }

    let total = candidates.len();
    for round in 1..=retry_rounds {
        if retry_rounds > 1 {
            log_runtime_line(
                format!("WiFi retry round {}/{}", round, retry_rounds).as_str(),
            );
        }

        for (i, (slot, ssid, pass, location)) in candidates.iter().enumerate() {
            let attempt_no = i + 1;
            if pass.trim().is_empty() {
                log_runtime_line(
                    format!("WiFi failover slot {} uses empty password", slot).as_str(),
                );
            }
            if retry_rounds > 1 {
                log_runtime_line(
                    format!(
                        "{}{} (round {}/{}, attempt {}/{})",
                        connect_context, ssid, round, retry_rounds, attempt_no, total
                    )
                    .as_str(),
                );
            } else {
                log_runtime_line(
                    format!("{}{} (attempt {}/{})", connect_context, ssid, attempt_no, total)
                        .as_str(),
                );
            }

            // Reset STA before switching credentials.
            wifi_sender.send(WifiCommand::StaDown).await;
            Timer::after(Duration::from_millis(150)).await;
            wifi_sender
                .send(WifiCommand::StaUp(ssid.clone(), pass.clone()))
                .await;

            let mut connected = wait_for_sta_network(stack, attempt_timeout).await;
            if !connected && wifi_sta_connected() {
                log_runtime_line("WiFi STA link is up; waiting a bit longer for IPv4...");
                connected = wait_for_sta_network(stack, Duration::from_secs(10)).await;
            }

            if connected {
                log_runtime_line(format!("WiFi connected: {}", ssid).as_str());
                return Some((*slot, ssid.clone(), location.clone()));
            }

            log_runtime_line("WiFi connect attempt failed, trying next (if any)...");
        }
    }

    None
}

// ═══════════════════════════════════════════════════════════════════════════
// NO-WIFI mode — sync DHT22 console logging
// ═══════════════════════════════════════════════════════════════════════════
#[cfg(not(feature = "wifi"))]
#[esp_hal::main]
fn main() -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);
    let delay = Delay::new();

    esp_alloc::heap_allocator!(size: 36 * 1024);
    esp_println::logger::init_logger_from_env();

    // Give SSD1306 modules a short power-up settle window on cold boot.
    delay.delay_millis(OLED_BOOT_INIT_RETRY_DELAY_MS);

    let oled_addr = oled_i2c_address();
    let mut oled = match I2c::new(
        peripherals.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(400)),
    ) {
        Ok(i2c) => {
            let mut i2c = i2c.with_sda(peripherals.GPIO6).with_scl(peripherals.GPIO7);
            let mut found_any = false;
            esp_println::println!("I2C scan start (GPIO6 SDA, GPIO7 SCL)...");
            for addr in 0x08u8..=0x77u8 {
                if i2c.write(addr, &[]).is_ok() {
                    found_any = true;
                    esp_println::println!("I2C: found device at 0x{:02X}", addr);
                }
            }
            if !found_any {
                esp_println::println!("I2C scan: no device ACK on 0x08..0x77");
            }
            Some(
                Ssd1306::new(
                    I2CDisplayInterface::new_custom_address(i2c, oled_addr),
                    DisplaySize128x32,
                    DisplayRotation::Rotate0,
                )
                .into_buffered_graphics_mode(),
            )
        }
        Err(e) => {
            esp_println::println!("OLED: I2C bus init failed: {:?}", e);
            None
        }
    };

    if let Some(display) = oled.as_mut() {
        let mut initialized = false;
        for attempt in 1..=OLED_BOOT_INIT_RETRIES {
            match oled_display::init_screen(display, "Booting...") {
                Ok(()) => {
                    esp_println::println!(
                        "OLED: init OK at I2C address 0x{:02X} (attempt {}/{})",
                        oled_addr,
                        attempt,
                        OLED_BOOT_INIT_RETRIES
                    );
                    initialized = true;
                    break;
                }
                Err(e) => {
                    esp_println::println!(
                        "OLED: init failed at I2C address 0x{:02X} (attempt {}/{}): {:?}",
                        oled_addr,
                        attempt,
                        OLED_BOOT_INIT_RETRIES,
                        e
                    );

                    if attempt < OLED_BOOT_INIT_RETRIES {
                        delay.delay_millis(OLED_BOOT_INIT_RETRY_DELAY_MS);
                    }
                }
            }
        }

        if !initialized {
            esp_println::println!(
                "OLED: init did not succeed at boot; keeping OLED active for runtime recovery"
            );
        }
    } else {
        esp_println::println!("OLED: disabled (bus init failed)");
    }

    let mut dht22_pin = Flex::new(peripherals.GPIO5);
    dht22_pin.apply_output_config(
        &OutputConfig::default()
            .with_drive_mode(DriveMode::OpenDrain)
            .with_pull(Pull::Up),
    );
    dht22_pin.set_output_enable(true);
    dht22_pin.set_input_enable(true);
    dht22_pin.set_high();

    let mut dht22 = Dht22::new(dht22_pin, Delay::new());

    loop {
        delay.delay_millis(10000);
        match dht22.read() {
            Ok(r) => {
                let temp = r.temperature - 1.7;
                let hum = (r.humidity - 7.0).max(0.0);
                esp_println::println!(
                    "DHT22 - Temperature: {:.1} °C, Humidity: {:.1} %",
                    temp, hum
                );
                if let Some(display) = oled.as_mut() {
                    let total_secs = HalInstant::now().duration_since_epoch().as_millis() / 1000;
                    let hours = ((total_secs / 3600) % 24) as u8;
                    let minutes = ((total_secs % 3600) / 60) as u8;
                    let _ = oled_display::show_reading(display, temp, hum, hours, minutes, false);
                }
            }
            Err(e) => {
                esp_println::dbg!("Sensor read error: {:?}", e);
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// WIFI mode — async embassy + raw TCP HTTP POST
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(feature = "wifi")]
type StatusLed = esp_hal::rmt::Channel<'static, esp_hal::Blocking, Tx>;

#[cfg(feature = "wifi")]
fn ws2812_encode_grb(g: u8, r: u8, b: u8) -> [PulseCode; 25] {
    // WS2812 timing at 80MHz RMT source clock and divider=1.
    const T0H: u16 = 28;
    const T0L: u16 = 56;
    const T1H: u16 = 64;
    const T1L: u16 = 48;

    let zero = PulseCode::new(esp_hal::gpio::Level::High, T0H, esp_hal::gpio::Level::Low, T0L);
    let one = PulseCode::new(esp_hal::gpio::Level::High, T1H, esp_hal::gpio::Level::Low, T1L);

    let mut out = [PulseCode::end_marker(); 25];
    let bytes = [g, r, b];
    let mut i = 0usize;
    for byte in bytes {
        for bit in (0..8).rev() {
            out[i] = if (byte & (1 << bit)) != 0 { one } else { zero };
            i += 1;
        }
    }

    out[24] = PulseCode::end_marker();
    out
}

#[cfg(feature = "wifi")]
fn ws2812_write(channel: StatusLed, r: u8, g: u8, b: u8) -> StatusLed {
    let data = ws2812_encode_grb(g, r, b);
    match channel.transmit(&data) {
        Ok(tx) => match tx.wait() {
            Ok(next_channel) => next_channel,
            Err((_, next_channel)) => next_channel,
        },
        Err((_, next_channel)) => next_channel,
    }
}

#[cfg(feature = "wifi")]
#[embassy_executor::task]
async fn led_task(mut channel: StatusLed) {
    let mut blink = false;
    let mut connect_level: i16 = 24;
    let mut connect_step: i16 = 14;
    let mut send_level: i16 = 24;
    let mut send_step: i16 = 12;

    loop {
        if POWER_SAVE_ACTIVE.load(Ordering::Relaxed) {
            // During sleep preparation/wait, keep RGB LED fully off.
            channel = ws2812_write(channel, 0, 0, 0);
            Timer::after(Duration::from_millis(120)).await;
        } else if WIFI_CONNECTING.load(Ordering::Relaxed) {
            // WiFi scan/connect in progress: orange breathing pulse.
            let amber_g = connect_level.clamp(24, 120) as u8;
            let amber_r = (connect_level * 2).clamp(48, 220) as u8;
            channel = ws2812_write(channel, amber_r, amber_g, 0);

            connect_level += connect_step;
            if connect_level >= 120 || connect_level <= 24 {
                connect_step = -connect_step;
            }
            Timer::after(Duration::from_millis(120)).await;
        } else if BOOTING.load(Ordering::Relaxed) {
            // Boot: blue
            channel = ws2812_write(channel, 0, 0, 96);
            Timer::after(Duration::from_millis(120)).await;
        } else if AP_ACTIVE.load(Ordering::Relaxed) {
            // AP mode: purple blink
            if blink {
                channel = ws2812_write(channel, 80, 0, 80);
            } else {
                channel = ws2812_write(channel, 0, 0, 0);
            }
            Timer::after(Duration::from_millis(300)).await;
            blink = !blink;
        } else if SENDING.load(Ordering::Relaxed) {
            // Sending: green breathing pulse (distinct from WiFi connect amber)
            let green = send_level.clamp(24, 140) as u8;
            channel = ws2812_write(channel, 0, green, 0);
            send_level += send_step;
            if send_level >= 140 || send_level <= 24 {
                send_step = -send_step;
            }
            Timer::after(Duration::from_millis(100)).await;
        } else {
            let now_secs = (Instant::now().as_millis() / 1000) as usize;
            let last_sent_secs = LAST_SENT_SECS.load(Ordering::Relaxed);
            let recently_sent = now_secs.saturating_sub(last_sent_secs) < LED_SENT_GREEN_WINDOW_SECS;

            if recently_sent {
                // Sent recently: green
                channel = ws2812_write(channel, 0, 80, 0);
                Timer::after(Duration::from_millis(120)).await;
            } else {
                // Idle: LED off
                channel = ws2812_write(channel, 0, 0, 0);
                Timer::after(Duration::from_millis(250)).await;
            }
        }
    }
}

#[cfg(feature = "wifi")]
#[embassy_executor::task]
async fn gpio_poll_task(btn: Input<'static>) {
    loop {
        Timer::after(Duration::from_millis(10)).await;
        BUTTON_PRESSED.store(btn.level() == Level::Low, Ordering::Relaxed);
    }
}

#[cfg(feature = "wifi")]
#[embassy_executor::task]
async fn button_task() {
    loop {
        // Wait for press.
        while !BUTTON_PRESSED.load(Ordering::Relaxed) {
            Timer::after(Duration::from_millis(20)).await;
        }
        let press_start = Instant::now();

        // Wait for release.
        while BUTTON_PRESSED.load(Ordering::Relaxed) {
            Timer::after(Duration::from_millis(20)).await;
        }
        let held = press_start.elapsed();

        if held >= Duration::from_secs(3) {
            BUTTON_EVENT.store(BUTTON_EVENT_LONG, Ordering::Relaxed);
        } else {
            BUTTON_EVENT.store(BUTTON_EVENT_SHORT, Ordering::Relaxed);
        }

        Timer::after(Duration::from_millis(300)).await;
    }
}

#[cfg(feature = "wifi")]
fn find_best_ssid(cfg: &AppConfig) -> Option<(usize, String, String, String)> {
    let slots = [
        (1usize, cfg.wifi_ssid.as_str(), cfg.wifi_password.as_str(), cfg.wifi_location.as_str()),
        (
            2usize,
            cfg.wifi_ssid_2.as_str(),
            cfg.wifi_password_2.as_str(),
            cfg.wifi_location_2.as_str(),
        ),
        (
            3usize,
            cfg.wifi_ssid_3.as_str(),
            cfg.wifi_password_3.as_str(),
            cfg.wifi_location_3.as_str(),
        ),
        (
            4usize,
            cfg.wifi_ssid_4.as_str(),
            cfg.wifi_password_4.as_str(),
            cfg.wifi_location_4.as_str(),
        ),
        (
            5usize,
            cfg.wifi_ssid_5.as_str(),
            cfg.wifi_password_5.as_str(),
            cfg.wifi_location_5.as_str(),
        ),
    ];
    // Prefer a network that is visible in the latest scan results.
    for (slot, ssid, pass, location) in slots {
        if !ssid.is_empty() && wifi_runtime::scan_ssid_visible(ssid) {
            return Some((
                slot,
                String::from(ssid),
                String::from(pass),
                String::from(location.trim()),
            ));
        }
    }
    // Fall back to the first configured slot.
    slots
        .into_iter()
        .find(|(_, ssid, _, _)| !ssid.is_empty())
        .map(|(slot, ssid, pass, location)| {
            (
                slot,
                String::from(ssid),
                String::from(pass),
                String::from(location.trim()),
            )
        })
}

#[cfg(feature = "wifi")]
#[esp_rtos::main]
async fn main(spawner: Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);
    let delay = Delay::new();

    esp_alloc::heap_allocator!(size: 72 * 1024);
    esp_println::logger::init_logger_from_env();

    // Give SSD1306 modules a short power-up settle window on cold boot.
    delay.delay_millis(OLED_BOOT_INIT_RETRY_DELAY_MS);

    // Start the RISC-V RTOS scheduler (required by esp_radio::init).
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_ints = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_ints.software_interrupt0);
    let mut lpwr_for_sleep = Some(peripherals.LPWR);

    // GPIO9 = BOOT button (active-low).
    let btn = Input::new(peripherals.GPIO9, InputConfig::default().with_pull(Pull::Up));
    let boot_button_held = btn.level() == Level::Low;
    spawner.spawn(gpio_poll_task(btn).unwrap());

    // GPIO8 = WS2812 RGB LED on ESP32-C6 Super Mini.
    let rmt = esp_hal::rmt::Rmt::new(peripherals.RMT, Rate::from_mhz(80)).expect("RMT init failed");
    let led_tx_cfg = TxChannelConfig::default()
        .with_clk_divider(1)
        .with_idle_output_level(Level::Low)
        .with_idle_output(true)
        .with_carrier_modulation(false);
    let led: StatusLed = rmt
        .channel0
        .configure_tx(&led_tx_cfg)
        .expect("RMT TX channel init failed")
        .with_pin(peripherals.GPIO8);
    spawner.spawn(led_task(led).unwrap());

    let mut flash = FlashStorage::new(peripherals.FLASH);

    let oled_addr = oled_i2c_address();
    let mut oled = if oled_enabled() {
        match I2c::new(
            peripherals.I2C0,
            I2cConfig::default().with_frequency(Rate::from_khz(400)),
        ) {
            Ok(i2c) => {
                let mut i2c = i2c.with_sda(peripherals.GPIO6).with_scl(peripherals.GPIO7);
                if oled_boot_scan_enabled() {
                    let mut found_any = false;
                    log_runtime_line("I2C scan start (GPIO6 SDA, GPIO7 SCL)...");
                    for addr in 0x08u8..=0x77u8 {
                        if i2c.write(addr, &[]).is_ok() {
                            found_any = true;
                            log_runtime_line(format!("I2C: found device at 0x{:02X}", addr).as_str());
                        }
                    }
                    if !found_any {
                        log_runtime_line("I2C scan: no device ACK on 0x08..0x77");
                    }
                } else {
                    log_runtime_line("I2C scan skipped (set OLED_BOOT_SCAN=1 to enable).");
                }
                Some(
                    Ssd1306::new(
                        I2CDisplayInterface::new_custom_address(i2c, oled_addr),
                        DisplaySize128x32,
                        DisplayRotation::Rotate0,
                    )
                    .into_buffered_graphics_mode(),
                )
            }
            Err(e) => {
                log_runtime_line(format!("OLED: I2C bus init failed: {:?}", e).as_str());
                None
            }
        }
    } else {
        log_runtime_line("OLED disabled (set OLED_ENABLED=1 to force-enable).");
        None
    };

    if let Some(display) = oled.as_mut() {
        let mut initialized = false;
        for attempt in 1..=OLED_BOOT_INIT_RETRIES {
            match oled_display::init_screen(display, "Booting WiFi...") {
                Ok(()) => {
                    log_runtime_line(
                        format!(
                            "OLED: init OK at I2C address 0x{:02X} (attempt {}/{})",
                            oled_addr, attempt, OLED_BOOT_INIT_RETRIES
                        )
                        .as_str(),
                    );
                    initialized = true;
                    break;
                }
                Err(e) => {
                    log_runtime_line(
                        format!(
                            "OLED: init failed at I2C address 0x{:02X} (attempt {}/{}): {:?}",
                            oled_addr, attempt, OLED_BOOT_INIT_RETRIES, e
                        )
                        .as_str(),
                    );

                    if attempt < OLED_BOOT_INIT_RETRIES {
                        delay.delay_millis(OLED_BOOT_INIT_RETRY_DELAY_MS);
                    }
                }
            }
        }

        if !initialized {
            log_runtime_line("OLED: init did not succeed at boot; runtime recovery remains enabled");
        }
    } else {
        log_runtime_line("OLED: disabled (bus init failed)");
    }

    let partition_range = resolve_partition_range(&mut flash, "config").expect("config partition missing");
    let mut config_storage = FlashConfigStorage::new(flash, partition_range);
    config_storage.mount().await.expect("config mount failed");
    let loaded_config = config_storage.load_config().await.expect("config load failed");

    if boot_button_held
        || !loaded_config.deep_sleep_enabled
        || is_power_bank_mode(&loaded_config)
        || deep_sleep_keep_alive_wake_interval_secs(&loaded_config) == 0
    {
        clear_keep_alive_sleep_plan();
    } else if has_keep_alive_sleep_plan() {
        let configured_interval_secs = sensor_interval_secs(&loaded_config);
        let wake_interval_secs = deep_sleep_keep_alive_wake_interval_secs(&loaded_config);
        let (sleep_chunk_secs, has_more_chunks) =
            next_keep_alive_sleep_chunk_secs(configured_interval_secs, wake_interval_secs);
        let pulse_ms = deep_sleep_keep_alive_pulse_ms(&loaded_config);

        log_runtime_line(
            format!(
                "Power-save: keep-alive wake chunk {} s (remaining chunk cycle: {}).",
                sleep_chunk_secs,
                if has_more_chunks { "yes" } else { "no" }
            )
            .as_str(),
        );

        if pulse_ms > 0 {
            log_runtime_line(
                format!(
                    "Power-save: keep-alive pulse {} ms on wake before returning to deep sleep.",
                    pulse_ms
                )
                .as_str(),
            );
            // Use a blocking delay so the CPU stays fully active during the pulse,
            // drawing enough current (~20-30 mA) to keep power banks from shutting off.
            // Timer::after() would allow the executor to light-sleep the CPU.
            Delay::new().delay_millis(pulse_ms as u32);
        }

        let wakeup = TimerWakeupSource::new(CoreDuration::from_secs(sleep_chunk_secs));
        if let Some(lpwr) = lpwr_for_sleep.take() {
            let mut rtc = Rtc::new(lpwr);
            rtc.sleep_deep(&[&wakeup]);
        }
    }

    let initial_oled_orientation = loaded_config.oled_orientation.clone();
    let config_handle: &'static Mutex<CriticalSectionRawMutex, AppConfig> =
        Box::leak(Box::new(Mutex::new(loaded_config)));
    let config_storage: &'static Mutex<CriticalSectionRawMutex, FlashConfigStorage<'static>> =
        Box::leak(Box::new(Mutex::new(config_storage)));

    if let Some(display) = oled.as_mut() {
        let rotation = oled_rotation_from_config(initial_oled_orientation.as_str());
        if let Err(e) = display.init() {
            log_runtime_line(format!("OLED: reinit before startup rotation failed: {:?}", e).as_str());
        }
        if let Err(e) = display.set_rotation(rotation) {
            log_runtime_line(format!("OLED: set_rotation failed: {:?}", e).as_str());
        }
        if let Err(e) = display.set_brightness(Brightness::BRIGHTEST) {
            log_runtime_line(format!("OLED: brightness restore failed: {:?}", e).as_str());
        }
    }

    // USB console for configuring from a computer.
    let usb_serial = esp_hal::usb_serial_jtag::UsbSerialJtag::new(peripherals.USB_DEVICE);
    spawner.spawn(usb_console::console_task(usb_serial, config_handle, config_storage).unwrap());

    log_runtime_line("Boot: initializing WiFi runtime...");
    let (wifi_stacks, wifi_sender) = wifi_runtime::init(&spawner, peripherals.WIFI).await;
    log_runtime_line("Boot: WiFi runtime initialized.");
    log_runtime_line("Boot: starting config portal tasks...");
    config_portal::start(spawner, wifi_stacks.ap, config_handle, config_storage);
    log_runtime_line("Boot: config portal tasks started.");
    let config_rx = config_portal::config_update_receiver();

    spawner.spawn(button_task().unwrap());
    log_runtime_line("Boot: button task started.");

    let stack: &'static Stack<'static> = NET_STACK.init(wifi_stacks.sta);
    let tcp_state: &'static TcpClientState<2, 4096, 4096> = TCP_STATE.init(TcpClientState::new());

    if espnow_gateway::key().is_some() {
        spawner.spawn(espnow_gateway::rx_task(wifi_stacks.esp_now, wifi_stacks.sta_mac).unwrap());
        spawner.spawn(espnow_gateway::forward_task(stack, tcp_state, config_handle).unwrap());
        if config_handle.lock().await.deep_sleep_enabled {
            log_runtime_line(
                "ESP-NOW: WARNING deep sleep is enabled; sensor packets are missed while the gateway sleeps.",
            );
        }
    } else {
        log_runtime_line("ESP-NOW: gateway disabled (build with ESPNOW_KEY, min 8 chars, to enable).");
    }
    let mut portal_mode = PortalMode::Off;
    let mut deep_sleep_blocked_by_manual_mode = boot_button_held;
    let mut active_wifi_location = String::new();
    let mut active_wifi_ssid = String::new();
    let mut active_wifi_slot: usize = 1;
    let mut oled_portrait_mode = oled_is_portrait(initial_oled_orientation.as_str());
    let mut trigger_sensor_now_after_boot = false;

    log_runtime_line(
        format!("Boot: button held at startup = {}", boot_button_held).as_str(),
    );

    if boot_button_held {
        wifi_sender.send(WifiCommand::StaDown).await;
        if let Some(display) = oled.as_mut() {
            draw_status_for_mode(display, "AP: 192.168.4.1", oled_portrait_mode);
        }
        set_portal_mode(
            &wifi_sender,
            &mut portal_mode,
            PortalMode::Manual,
            "Boot: button held -> forcing AP portal at 192.168.4.1",
        )
        .await;
        deep_sleep_blocked_by_manual_mode = true;
    }

    // ── Boot: one STA attempt, then AP fallback ─────────────────────────────
    // Some APs take longer for association + DHCP; keep this generous to avoid
    // rapid failover loops.
    const BOOT_CONNECT_TIMEOUT_SECS: u64 = 25;

    if portal_mode == PortalMode::Off {
        let cfg_snapshot = { config_handle.lock().await.clone() };
        if has_wifi_config(&cfg_snapshot) {
            if let Some(display) = oled.as_mut() {
                draw_status_for_mode(display, "WiFi scan...", oled_portrait_mode);
            }
            log_runtime_line("Boot: starting initial WiFi scan/connect...");
            if let Some((slot, ssid, location)) = scan_and_connect_best_configured_wifi(
                &wifi_sender,
                &cfg_snapshot,
                "Boot: scanning visible WiFi networks before connect...",
                "Boot: connecting to ",
            )
            .await {
                active_wifi_slot = slot;
                active_wifi_ssid = ssid.clone();
                active_wifi_location = location;
                if let Some(display) = oled.as_mut() {
                    draw_status_for_mode(
                        display,
                        format!("Connecting {}", ssid).as_str(),
                        oled_portrait_mode,
                    );
                }
            }
        } else {
            log_runtime_line("Boot: no saved WiFi config, opening AP portal.");
            wifi_sender.send(WifiCommand::StaDown).await;
            if let Some(display) = oled.as_mut() {
                draw_status_for_mode(display, "AP: 192.168.4.1", oled_portrait_mode);
            }
            set_portal_mode(
                &wifi_sender,
                &mut portal_mode,
                PortalMode::MissingConfig,
                "No WiFi config: starting AP portal at 192.168.4.1",
            )
            .await;
        }
    }

    if portal_mode == PortalMode::Off {
        log_runtime_line("Boot: waiting up to 15 s for STA connection...");
        if let Some(display) = oled.as_mut() {
            let status = if active_wifi_ssid.is_empty() {
                format!("Connecting...")
            } else {
                format!("Connecting {}", active_wifi_ssid)
            };
            draw_status_for_mode(display, status.as_str(), oled_portrait_mode);
        }
        let mut boot_connected =
            wait_for_sta_network(stack, Duration::from_secs(BOOT_CONNECT_TIMEOUT_SECS)).await;

        if !boot_connected && wifi_sta_connected() {
            log_runtime_line("WiFi STA link is up; waiting a bit longer for IPv4...");
            boot_connected = wait_for_sta_network(stack, Duration::from_secs(10)).await;
        }

        if !boot_connected {
            log_runtime_line("Boot: STA connect timed out; trying other configured WiFi networks...");
            let cfg_snapshot = { config_handle.lock().await.clone() };
            if let Some((slot, ssid, location)) = connect_configured_wifi_with_failover(
                &wifi_sender,
                stack,
                &cfg_snapshot,
                active_wifi_ssid.as_str(),
                "Boot: scanning visible WiFi networks for failover...",
                "Boot: trying ",
                Duration::from_secs(BOOT_CONNECT_TIMEOUT_SECS),
            )
            .await
            {
                active_wifi_slot = slot;
                active_wifi_ssid = ssid;
                active_wifi_location = location;
                boot_connected = true;
            }
        }

        if boot_connected {
            log_runtime_line("WiFi connected during boot.");
            if let Some(cfg) = stack.config_v4() {
                log_runtime_line(format!("Got IP: {:?}", cfg.address).as_str());
            }
            // Do not wait SENSOR_INTERVAL on first connected boot; read/post immediately.
            trigger_sensor_now_after_boot = true;
        } else {
            wifi_sender.send(WifiCommand::StaDown).await;
            if let Some(display) = oled.as_mut() {
                draw_status_for_mode(display, "AP: 192.168.4.1", oled_portrait_mode);
            }
            set_portal_mode(
                &wifi_sender,
                &mut portal_mode,
                PortalMode::Recovery,
                "Boot: STA connect timeout, starting AP portal at 192.168.4.1",
            )
            .await;
        }
    }

    BOOTING.store(false, Ordering::Relaxed);

    // DHT22 on GPIO5 (open-drain, pull-up).
    let mut dht22_pin = Flex::new(peripherals.GPIO5);
    dht22_pin.apply_output_config(
        &OutputConfig::default()
            .with_drive_mode(DriveMode::OpenDrain)
            .with_pull(Pull::Up),
    );
    dht22_pin.set_output_enable(true);
    dht22_pin.set_input_enable(true);
    dht22_pin.set_high();

    let mut dht22 = Dht22::new(dht22_pin, Delay::new());

    // Cache last sensor readings for WiFi display; None means no measurement yet.
    let mut last_temp: Option<f32> = None;
    let mut last_hum: Option<f32> = None;

    let mut net_was_up = stack_has_ipv4(stack);
    let configured_sensor_interval_secs = {
        let cfg = config_handle.lock().await;
        sensor_interval_secs(&cfg)
    };
    let mut next_sensor_at = Instant::now() + Duration::from_secs(configured_sensor_interval_secs);
    if trigger_sensor_now_after_boot {
        next_sensor_at = Instant::now();
    }
    let mut next_oled_refresh_at = Instant::now();
    let mut last_oled_data_sent = false;
    let mut oled_page: usize = 0;
    let mut next_page_switch_at = Instant::now() + Duration::from_millis(OLED_PAGE_MS);
    let mut last_remote_generation = espnow_gateway::display_generation();
    loop {
        ACTIVE_WIFI_SLOT.store(active_wifi_slot, Ordering::Relaxed);
        let config_save_changed = embassy_time::with_timeout(
            Duration::from_millis(MAIN_LOOP_TICK_MS),
            config_rx.receive(),
        )
        .await
        .ok();

        if let Some(event) = take_button_event() {
            match event {
                ButtonEvent::LongPress => {
                    if let Some(display) = oled.as_mut() {
                        draw_status_for_mode(display, "AP: 192.168.4.1", oled_portrait_mode);
                    }
                    set_portal_mode(
                        &wifi_sender,
                        &mut portal_mode,
                        PortalMode::Manual,
                        "Button: long press -> AP portal opening (AP-only, channel override active): http://192.168.4.1/",
                    )
                    .await;
                    deep_sleep_blocked_by_manual_mode = true;
                }
                ButtonEvent::ShortPress => {
                    if portal_mode.is_open() {
                        set_portal_mode(
                            &wifi_sender,
                            &mut portal_mode,
                            PortalMode::Off,
                            "Button: short press -> AP portal closed",
                        )
                        .await;

                        if stack_has_ipv4(stack) {
                            log_runtime_line("Button: STA already connected, skipping reconnect.");
                            if let Some(display) = oled.as_mut() {
                                draw_status_for_mode(
                                    display,
                                    format!("WiFi {}", active_wifi_ssid).as_str(),
                                    oled_portrait_mode,
                                );
                            }
                            next_sensor_at = Instant::now();
                            deep_sleep_blocked_by_manual_mode = false;
                        } else {
                            let cfg_snapshot = { config_handle.lock().await.clone() };
                            if has_wifi_config(&cfg_snapshot) {
                                if let Some((slot, ssid, location)) = reconnect_after_portal_close(
                                    &wifi_sender,
                                    stack,
                                    &cfg_snapshot,
                                    active_wifi_ssid.as_str(),
                                    "Button:",
                                )
                                .await
                                {
                                    active_wifi_slot = slot;
                                    active_wifi_ssid = ssid.clone();
                                    active_wifi_location = location;
                                    if let Some(display) = oled.as_mut() {
                                        draw_status_for_mode(
                                            display,
                                            format!("WiFi {}", ssid).as_str(),
                                            oled_portrait_mode,
                                        );
                                    }
                                    next_sensor_at = Instant::now();
                                } else {
                                    if let Some(display) = oled.as_mut() {
                                        draw_status_for_mode(display, "AP: 192.168.4.1", oled_portrait_mode);
                                    }
                                    set_portal_mode(
                                        &wifi_sender,
                                        &mut portal_mode,
                                        PortalMode::Recovery,
                                        "Button: reconnect failed; reopening AP portal for recovery.",
                                    )
                                    .await;
                                }
                            }
                        }
                    } else {
                        if let Some(display) = oled.as_mut() {
                            draw_status_for_mode(display, "AP: 192.168.4.1", oled_portrait_mode);
                        }
                        set_portal_mode(
                            &wifi_sender,
                            &mut portal_mode,
                            PortalMode::Manual,
                            "Button: short press -> AP portal opening (AP-only, channel override active): http://192.168.4.1/",
                        )
                        .await;
                        deep_sleep_blocked_by_manual_mode = true;
                    }
                }
            }
        }

        let net_is_up_now = stack_has_ipv4(stack);
        if net_is_up_now && !net_was_up {
            log_runtime_line("WiFi connected");
            if deep_sleep_blocked_by_manual_mode && portal_mode == PortalMode::Off {
                deep_sleep_blocked_by_manual_mode = false;
                log_runtime_line("Power-save: normal mode restored, deep sleep unlocked.");
            }
            if let Some(cfg) = stack.config_v4() {
                log_runtime_line(format!("Got IP: {:?}", cfg.address).as_str());
                if let Some(display) = oled.as_mut() {
                    if portal_mode.is_open() {
                        // Keep AP status page visible while portal is open.
                    } else {
                    let tick = (Instant::now().as_millis() / OLED_REFRESH_MS as u64) as usize;
                    let wifi_label = wifi_display_label(
                        active_wifi_location.as_str(),
                        active_wifi_ssid.as_str(),
                    );
                    let _ = oled_display::show_reading_with_wifi(
                        display,
                        last_temp,
                        last_hum,
                        wifi_label.as_str(),
                        false,
                        tick,
                        oled_portrait_mode,
                    );
                    last_oled_data_sent = false;
                    }
                }
            }
            next_sensor_at = Instant::now();
        } else if !net_is_up_now && net_was_up {
            log_runtime_line("WiFi disconnected");
            if let Some(display) = oled.as_mut() {
                if portal_mode.is_open() {
                    draw_status_for_mode(display, "AP: 192.168.4.1", oled_portrait_mode);
                } else {
                    draw_status_for_mode(display, "WiFi disconnected", oled_portrait_mode);
                }
            }
        }
        net_was_up = net_is_up_now;

        if let Some(changed) = config_save_changed {
            let (has_config, pending_oled_orientation) = {
                let cfg = config_handle.lock().await;
                let pending_oled_orientation = cfg.oled_orientation.clone();
                if has_wifi_config(&cfg) {
                    let should_reconnect = !stack_has_ipv4(stack);
                    if should_reconnect {
                        if let Some((slot, ssid, location)) = scan_and_connect_best_configured_wifi(
                            &wifi_sender,
                            &cfg,
                            "Config saved: scanning visible WiFi networks before connect...",
                            "Config saved: connecting to ",
                        )
                        .await {
                            active_wifi_slot = slot;
                            active_wifi_ssid = ssid.clone();
                            active_wifi_location = location;
                            if let Some(display) = oled.as_mut() {
                                draw_status_for_mode(
                                    display,
                                    format!("Connecting {}", ssid).as_str(),
                                    oled_portrait_mode,
                                );
                            }
                        }
                    } else {
                        if changed {
                            log_runtime_line("Config saved: WiFi already connected, skipping scan/reconnect.");
                        } else {
                            log_runtime_line("Config saved: WiFi already connected, no changes.");
                        }
                    }
                    (true, pending_oled_orientation)
                } else {
                    (false, pending_oled_orientation)
                }
            };

            if changed {
                if let Some(display) = oled.as_mut() {
                    let rotation = oled_rotation_from_config(pending_oled_orientation.as_str());
                    oled_portrait_mode = oled_is_portrait(pending_oled_orientation.as_str());
                    if let Err(e) = display.init() {
                        log_runtime_line(format!("OLED: reinit before rotation failed: {:?}", e).as_str());
                    }
                    if let Err(e) = display.set_rotation(rotation) {
                        log_runtime_line(format!("OLED: set_rotation failed: {:?}", e).as_str());
                    }
                    if let Err(e) = display.set_brightness(Brightness::BRIGHTEST) {
                        log_runtime_line(format!("OLED: brightness restore failed: {:?}", e).as_str());
                    }
                }
            }

            if portal_mode.is_open() && has_config {
                log_runtime_line("Config saved: closing AP portal in 2 s...");
                Timer::after(Duration::from_secs(SAVE_AP_CLOSE_DELAY_SECS)).await;
                set_portal_mode(
                    &wifi_sender,
                    &mut portal_mode,
                    PortalMode::Off,
                    "Config saved: AP portal closed, continuing WiFi connect.",
                )
                .await;

                if !stack_has_ipv4(stack) {
                    let cfg_snapshot = { config_handle.lock().await.clone() };
                    if has_wifi_config(&cfg_snapshot) {
                        if let Some((slot, ssid, location)) = reconnect_after_portal_close(
                            &wifi_sender,
                            stack,
                            &cfg_snapshot,
                            active_wifi_ssid.as_str(),
                            "Config save:",
                        )
                        .await
                        {
                            active_wifi_slot = slot;
                            active_wifi_ssid = ssid;
                            active_wifi_location = location;
                        } else {
                            if let Some(display) = oled.as_mut() {
                                draw_status_for_mode(display, "AP: 192.168.4.1", oled_portrait_mode);
                            }
                            set_portal_mode(
                                &wifi_sender,
                                &mut portal_mode,
                                PortalMode::Recovery,
                                "Config save: reconnect failed; AP portal reopened for recovery.",
                            )
                            .await;
                        }
                    }
                }
            } else if !has_config {
                wifi_sender.send(WifiCommand::StaDown).await;
                if let Some(display) = oled.as_mut() {
                    draw_status_for_mode(display, "AP: 192.168.4.1", oled_portrait_mode);
                }
                set_portal_mode(
                    &wifi_sender,
                    &mut portal_mode,
                    PortalMode::MissingConfig,
                    "Config saved without WiFi credentials: AP portal remains available.",
                )
                .await;
            }

            next_sensor_at = Instant::now();
            continue;
        }

        let now = Instant::now();
        if now >= next_oled_refresh_at && stack_has_ipv4(stack) && !portal_mode.is_open() {
            // Page 0 = own reading, page N = N-th ESP-NOW sensor heard recently.
            // A new remote reading jumps to its page; otherwise pages rotate.
            let (remote_pages, newest_remote) = espnow_gateway::display_pages();
            let remote_generation = espnow_gateway::display_generation();
            let remote_updated = remote_generation != last_remote_generation;
            let previous_page = oled_page;
            if remote_updated {
                last_remote_generation = remote_generation;
                if let Some(index) = newest_remote {
                    oled_page = index + 1;
                    next_page_switch_at = now + Duration::from_millis(OLED_REMOTE_HOLD_MS);
                }
            } else if now >= next_page_switch_at {
                oled_page = (oled_page + 1) % (remote_pages.len() + 1);
                next_page_switch_at = now + Duration::from_millis(OLED_PAGE_MS);
            }
            if oled_page > remote_pages.len() {
                oled_page = 0;
            }
            let page_changed = oled_page != previous_page;

            if oled_page > 0 {
                next_oled_refresh_at = now + Duration::from_millis(OLED_REFRESH_MS);
                if page_changed || remote_updated {
                    if let Some(display) = oled.as_mut() {
                        let page = &remote_pages[oled_page - 1];
                        let _ = oled_display::show_remote_reading(
                            display,
                            page.name.as_str(),
                            page.temp,
                            page.hum,
                            page.battery_pct,
                            page.footer.as_str(),
                            oled_portrait_mode,
                        );
                    }
                }
            } else {
            let total_secs = (now.as_millis() / 1000) as usize;
            let last_sent_secs = LAST_SENT_SECS.load(Ordering::Relaxed);
            let data_sent = total_secs.saturating_sub(last_sent_secs) < 5;
            let wifi_label = wifi_display_label(
                active_wifi_location.as_str(),
                active_wifi_ssid.as_str(),
            );
            let wifi_width = if data_sent { 16 } else { 21 };
            let should_scroll = !oled_portrait_mode && wifi_label.len() > wifi_width;
            let sent_state_changed = data_sent != last_oled_data_sent;

            // Periodic redraw is only needed for marquee scrolling, transient SENT
            // indicator, or when returning from a remote sensor page.
            if should_scroll || data_sent || sent_state_changed || page_changed {
                next_oled_refresh_at = now + Duration::from_millis(OLED_REFRESH_MS);
                if let Some(display) = oled.as_mut() {
                    let tick = (now.as_millis() / OLED_REFRESH_MS as u64) as usize;
                    let _ = oled_display::show_reading_with_wifi(
                        display,
                        last_temp,
                        last_hum,
                        wifi_label.as_str(),
                        data_sent,
                        tick,
                        oled_portrait_mode,
                    );
                    last_oled_data_sent = data_sent;
                }
            } else {
                // No animation needed; check again later with low frequency.
                next_oled_refresh_at = now + Duration::from_millis(OLED_REFRESH_MS);
            }
            }
        }

        if Instant::now() < next_sensor_at {
            continue;
        }
        let configured_sensor_interval_secs = {
            let cfg = config_handle.lock().await;
            sensor_interval_secs(&cfg)
        };
        next_sensor_at = Instant::now() + Duration::from_secs(configured_sensor_interval_secs);

        if wifi_runtime::scan_in_progress() {
            log_runtime_line("Scan in progress: pausing sensor post this cycle");
            continue;
        }

        if portal_mode.is_open() && !stack_has_ipv4(stack) {
            log_runtime_line("AP active: STA down, sensor post paused");
            continue;
        } else if portal_mode.is_open() && stack_has_ipv4(stack) {
            log_runtime_line("AP active: STA up, sensor post continues");
        }

        match dht22.read() {
            Ok(r) => {
                let (
                    temp_offset,
                    hum_offset,
                    api_url,
                    api_key,
                    mqtt_enabled,
                    mqtt_host,
                    mqtt_port,
                    mqtt_username,
                    mqtt_password,
                    mqtt_base_topic,
                    mqtt_device_id,
                ) = {
                    let cfg = config_handle.lock().await;
                    let t: f32 = cfg.temp_offset.parse().unwrap_or(-1.7);
                    let h: f32 = cfg.hum_offset.parse().unwrap_or(-7.0);
                    let mqtt_enabled_for_slot = match active_wifi_slot {
                        2 => cfg.mqtt_enabled_2,
                        3 => cfg.mqtt_enabled_3,
                        4 => cfg.mqtt_enabled_4,
                        5 => cfg.mqtt_enabled_5,
                        _ => cfg.mqtt_enabled_1,
                    };
                    (
                        t,
                        h,
                        cfg.api_url.clone(),
                        cfg.api_key.clone(),
                        mqtt_enabled_for_slot,
                        cfg.mqtt_host.clone(),
                        cfg.mqtt_port.clone(),
                        cfg.mqtt_username.clone(),
                        cfg.mqtt_password.clone(),
                        cfg.mqtt_base_topic.clone(),
                        cfg.mqtt_device_id.clone(),
                    )
                };
                let temp = r.temperature + temp_offset;
                let hum = (r.humidity + hum_offset).max(0.0);
                
                // Cache readings for WiFi display
                last_temp = Some(temp);
                last_hum = Some(hum);
                
                log_runtime_line(
                    format!(
                        "DHT22: {:.1} C  {:.1} %  (offsets: temp {:+.1} C, hum {:+.1} %)",
                        temp, hum, temp_offset, hum_offset
                    )
                    .as_str(),
                );
                if let Some(display) = oled.as_mut() {
                    if portal_mode.is_open() {
                        // Keep AP/portal status visible; do not overwrite with sensor data.
                    } else {
                    let total_secs = (Instant::now().as_millis() / 1000) as usize;
                    let last_sent_secs = LAST_SENT_SECS.load(Ordering::Relaxed);
                    let data_sent = total_secs.saturating_sub(last_sent_secs) < 5;
                    let wifi_label = wifi_display_label(
                        active_wifi_location.as_str(),
                        active_wifi_ssid.as_str(),
                    );
                    let _ = oled_display::show_reading_with_wifi(
                        display,
                        Some(temp),
                        Some(hum),
                        wifi_label.as_str(),
                        data_sent,
                        (Instant::now().as_millis() / OLED_REFRESH_MS as u64) as usize,
                        oled_portrait_mode,
                    );
                    last_oled_data_sent = data_sent;
                    // Own reading is on screen now; restart rotation from page 0.
                    oled_page = 0;
                    next_page_switch_at = Instant::now() + Duration::from_millis(OLED_PAGE_MS);
                    }
                }
                if stack_has_ipv4(stack) {
                    if mqtt_enabled && !mqtt_host.trim().is_empty() {
                        mqtt_publish_with_discovery(
                            *stack,
                            mqtt_host.as_str(),
                            mqtt_port.as_str(),
                            mqtt_username.as_str(),
                            mqtt_password.as_str(),
                            mqtt_base_topic.as_str(),
                            mqtt_device_id.as_str(),
                            active_wifi_location.as_str(),
                            temp,
                            hum,
                        )
                        .await;
                    }

                    post_readings_with_retry(
                        stack,
                        tcp_state,
                        api_url.as_str(),
                        api_key.as_str(),
                        active_wifi_location.as_str(),
                        temp,
                        hum,
                    )
                    .await;
                } else {
                    log_runtime_line("Network down, waiting for reconnect...");
                    let mut reconnected =
                        wait_for_sta_network(stack, Duration::from_secs(60)).await;
                    if !reconnected && wifi_sta_connected() {
                        log_runtime_line("WiFi STA link is back; waiting a bit longer for IPv4...");
                        reconnected = wait_for_sta_network(stack, Duration::from_secs(10)).await;
                    }
                    if reconnected {
                        log_runtime_line("Reconnected, waiting 5 s before sending...");
                        Timer::after(Duration::from_secs(5)).await;
                        post_readings_with_retry(
                            stack,
                            tcp_state,
                            api_url.as_str(),
                            api_key.as_str(),
                            active_wifi_location.as_str(),
                            temp,
                            hum,
                        )
                        .await;
                    } else {
                        log_runtime_line(
                            "Reconnect timed out; scanning and trying other configured WiFi networks...",
                        );
                        let cfg_snapshot = { config_handle.lock().await.clone() };
                        if let Some((slot, ssid, location)) = connect_configured_wifi_with_failover(
                            &wifi_sender,
                            stack,
                            &cfg_snapshot,
                            active_wifi_ssid.as_str(),
                            "Reconnect: scanning visible WiFi networks for failover...",
                            "Reconnect: trying ",
                            Duration::from_secs(25),
                        )
                        .await
                        {
                            active_wifi_slot = slot;
                            active_wifi_ssid = ssid;
                            active_wifi_location = location;
                            log_runtime_line("Failover connected, waiting 5 s before sending...");
                            Timer::after(Duration::from_secs(5)).await;
                            post_readings_with_retry(
                                stack,
                                tcp_state,
                                api_url.as_str(),
                                api_key.as_str(),
                                active_wifi_location.as_str(),
                                temp,
                                hum,
                            )
                            .await;
                        } else {
                            wifi_sender.send(WifiCommand::StaDown).await;
                            set_portal_mode(
                                &wifi_sender,
                                &mut portal_mode,
                                PortalMode::Recovery,
                                "Reconnect failed across configured networks; opening AP portal for recovery.",
                            )
                            .await;
                        }
                    }
                }

                let (sleep_interval_secs, keep_alive_pulse_ms, use_power_bank_mode, wait_oled_mode): (u64, u64, bool, alloc::string::String) = {
                    let cfg = config_handle.lock().await;
                    if cfg.deep_sleep_enabled {
                        (
                            sensor_interval_secs(&cfg),
                            deep_sleep_keep_alive_pulse_ms(&cfg),
                            is_power_bank_mode(&cfg),
                            oled_wait_mode(&cfg).into(),
                        )
                    } else {
                        (0, 0, false, "on".into())
                    }
                };

                let usb_host = sleep_interval_secs > 0 && usb_console::usb_host_present();
                if usb_host {
                    log_runtime_line("Power-save: computer on USB, staying awake for the console.");
                }
                if sleep_interval_secs > 0
                    && !portal_mode.is_open()
                    && !deep_sleep_blocked_by_manual_mode
                    && !usb_host
                {
                    if use_power_bank_mode {
                        POWER_SAVE_ACTIVE.store(true, Ordering::Relaxed);
                        // Power bank mode: use timer wait instead of deep sleep so the WiFi modem
                        // stays in modem-sleep drawing ~20-50 mA, enough to keep power banks alive.
                        // Deep sleep would draw only ~20 µA which triggers power bank auto-shutoff.
                        clear_keep_alive_sleep_plan();
                        log_runtime_line(
                            format!(
                                "Power-save: modem sleep {} s (power bank mode).",
                                sleep_interval_secs
                            )
                            .as_str(),
                        );
                        if let Some(display) = oled.as_mut() {
                            match wait_oled_mode.as_str() {
                                "off" => { let _ = display.set_display_on(false); },
                                "dim" => { let _ = display.set_brightness(Brightness::DIMMEST); },
                                _ => {}
                            }
                        }
                        Timer::after(Duration::from_secs(sleep_interval_secs)).await;
                        POWER_SAVE_ACTIVE.store(false, Ordering::Relaxed);
                        if let Some(display) = oled.as_mut() {
                            match wait_oled_mode.as_str() {
                                "off" => {
                                    let _ = display.set_display_on(true);
                                    let _ = display.set_brightness(Brightness::BRIGHTEST);
                                }
                                "dim" => { let _ = display.set_brightness(Brightness::BRIGHTEST); }
                                _ => {}
                            }
                        }
                    } else {
                        let (sleep_chunk_secs, _has_more_chunks) =
                            next_keep_alive_sleep_chunk_secs(sleep_interval_secs, 0);
                        log_runtime_line(
                            format!(
                                "Power-save: entering deep sleep for {} s.",
                                sleep_chunk_secs,
                            )
                            .as_str(),
                        );
                        POWER_SAVE_ACTIVE.store(true, Ordering::Relaxed);
                        if let Some(display) = oled.as_mut() {
                            match wait_oled_mode.as_str() {
                                "off" => {
                                    if let Err(e) = oled_display::prepare_for_sleep(display) {
                                        log_runtime_line(format!("OLED: sleep prepare failed: {:?}", e).as_str());
                                    }
                                }
                                "dim" => {
                                    if let Err(e) = display.set_brightness(Brightness::DIMMEST) {
                                        log_runtime_line(format!("OLED: dim before deep sleep failed: {:?}", e).as_str());
                                    }
                                }
                                _ => {}
                            }
                        }
                        if keep_alive_pulse_ms > 0 {
                            log_runtime_line(
                                format!(
                                    "Power-save: keep-alive pulse {} ms before deep sleep.",
                                    keep_alive_pulse_ms
                                )
                                .as_str(),
                            );
                            Delay::new().delay_millis(keep_alive_pulse_ms as u32);
                        }
                        Timer::after(Duration::from_millis(700)).await;
                        wifi_sender.send(WifiCommand::StaDown).await;
                        Timer::after(Duration::from_millis(200)).await;
                        let wakeup = TimerWakeupSource::new(CoreDuration::from_secs(sleep_chunk_secs));
                        if let Some(lpwr) = lpwr_for_sleep.take() {
                            let mut rtc = Rtc::new(lpwr);
                            rtc.sleep_deep(&[&wakeup]);
                        } else {
                            POWER_SAVE_ACTIVE.store(false, Ordering::Relaxed);
                            log_runtime_line("Power-save: LPWR already consumed; deep sleep skipped.");
                        }
                    }
                }
            }
            Err(e) => {
                log_runtime_line(format!("DHT22 read error: {:?}", e).as_str());
            }
        }
    }
}

/// Retry wrapper: tries up to 3 times with 10 s delay between attempts.
#[cfg(feature = "wifi")]
async fn post_readings_with_retry(
    stack: &'static Stack<'static>,
    tcp_state: &'static TcpClientState<2, 4096, 4096>,
    api_url: &str,
    api_key: &str,
    location: &str,
    temp: f32,
    hum: f32,
) -> bool {
    SENDING.store(true, Ordering::Relaxed);
    for attempt in 1..=3u8 {
        if attempt > 1 {
            log_runtime_line(format!("Retry {}/3 in 10 s...", attempt).as_str());
            Timer::after(Duration::from_secs(10)).await;
        }
        if post_readings(stack, tcp_state, api_url, api_key, location, temp, hum).await {
            SENDING.store(false, Ordering::Relaxed);
            let secs = (Instant::now().as_millis() / 1000) as usize;
            LAST_SENT_SECS.store(secs, Ordering::Relaxed);
            return true;
        }
    }
    SENDING.store(false, Ordering::Relaxed);
    log_runtime_line("All 3 POST attempts failed, will retry next cycle.");
    false
}

/// Send temperature + humidity as HTTPS JSON POST to the backend API.
/// Returns true on success.
#[cfg(feature = "wifi")]
async fn post_readings(
    stack: &'static Stack<'static>,
    tcp_state: &'static TcpClientState<2, 4096, 4096>,
    api_url: &str,
    api_key: &str,
    location: &str,
    temp: f32,
    hum: f32,
) -> bool {
    if api_url.is_empty() || api_key.is_empty() {
        log_runtime_line("API config missing, skipping POST.");
        return false;
    }

    let tcp_client = embassy_net::tcp::client::TcpClient::new(*stack, tcp_state);
    let dns = DnsSocket::new(*stack);

    #[cfg(not(feature = "no-tls"))]
    let tls_seed = esp_hal::rng::Rng::new().random() as u64;
    // Must hold a full TLS record (16 KiB + overhead): Let's Encrypt's chain
    // alone is ~4.3 KB, so a 4 KiB buffer fails with Tls(InsufficientSpace).
    #[cfg(not(feature = "no-tls"))]
    let mut tls_rx = [0u8; 16640];
    #[cfg(not(feature = "no-tls"))]
    let mut tls_tx = [0u8; 4096];
    #[cfg(not(feature = "no-tls"))]
    let tls = TlsConfig::new(tls_seed, &mut tls_rx, &mut tls_tx, TlsVerify::None);

    #[cfg(not(feature = "no-tls"))]
    let mut http = HttpClient::new_with_tls(&tcp_client, &dns, tls);
    #[cfg(feature = "no-tls")]
    let mut http = HttpClient::new(&tcp_client, &dns);

    let payload_location = location.trim();
    let body = if payload_location.is_empty() {
        format!(
            r#"[{{"sensor_type":"temperature","value":{:.2},"unit":"C"}},{{"sensor_type":"humidity","value":{:.2},"unit":"%"}}]"#,
            temp, hum
        )
    } else {
        let escaped_location = payload_location.replace('"', "\\\"");
        format!(
            r#"[{{"sensor_type":"temperature","value":{:.2},"unit":"C","location":"{}"}},{{"sensor_type":"humidity","value":{:.2},"unit":"%","location":"{}"}}]"#,
            temp,
            escaped_location,
            hum,
            escaped_location,
        )
    };
    let readings_url = format!("{}readings", api_url);

    let mut rx_buf = [0u8; 512];
    match http.request(Method::POST, readings_url.as_str()).await {
        Ok(req) => {
            let headers = [("X-API-Key", api_key)];
            let mut builder = req
                .content_type(ContentType::ApplicationJson)
                .headers(&headers)
                .body(body.as_bytes());
            let result = builder.send(&mut rx_buf).await;
            match result {
                Ok(r) => {
                    log_runtime_line(format!("POST -> HTTP {}", r.status.0).as_str());
                    true
                }
                Err(e) => {
                    log_runtime_line(format!("POST error: {:?}", e).as_str());
                    false
                }
            }
        }
        Err(e) => {
            log_runtime_line(format!("HTTP connect error: {:?}", e).as_str());
            false
        }
    }
}

// ── MQTT (Home Assistant Discovery) ─────────────────────────────────────────

#[cfg(feature = "wifi")]
const HA_DISCOVERY_PREFIX: &str = "homeassistant";

#[cfg(feature = "wifi")]
const MQTT_DNS_TIMEOUT_SECS: u64 = 5;
#[cfg(feature = "wifi")]
const MQTT_CONNECT_TIMEOUT_SECS: u64 = 5;
#[cfg(feature = "wifi")]
const MQTT_IO_TIMEOUT_SECS: u64 = 3;

#[cfg(feature = "wifi")]
fn sanitize_mqtt_id(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let mut out = String::with_capacity(trimmed.len());
    for ch in trimmed.chars() {
        let ch = ch.to_ascii_lowercase();
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else if ch.is_whitespace() {
            out.push('_');
        }
    }

    while out.contains("__") {
        out = out.replace("__", "_");
    }

    out.trim_matches('_').into()
}

#[cfg(feature = "wifi")]
fn mqtt_device_id(device_id_cfg: &str, wifi_location: &str) -> String {
    let from_cfg = sanitize_mqtt_id(device_id_cfg);
    if !from_cfg.is_empty() {
        return from_cfg;
    }

    let from_location = sanitize_mqtt_id(wifi_location);
    if !from_location.is_empty() {
        return from_location;
    }

    "climate-monitor".into()
}

#[cfg(feature = "wifi")]
fn parse_mqtt_port(port: &str) -> u16 {
    port.trim()
        .parse::<u16>()
        .ok()
        .filter(|p| *p > 0)
        .unwrap_or(1883)
}

#[cfg(feature = "wifi")]
async fn resolve_host_to_ip(stack: Stack<'static>, host: &str) -> Option<IpAddress> {
    if let Ok(ipv4) = host.parse::<Ipv4Addr>() {
        let octets = ipv4.octets();
        return Some(IpAddress::Ipv4(Ipv4Address::new(
            octets[0], octets[1], octets[2], octets[3],
        )));
    }

    let dns = DnsSocket::new(stack);
    let addrs = match embassy_time::with_timeout(
        Duration::from_secs(MQTT_DNS_TIMEOUT_SECS),
        dns.query(host, DnsQueryType::A),
    )
    .await
    {
        Ok(Ok(addrs)) => addrs,
        _ => return None,
    };
    addrs.get(0).copied()
}

#[cfg(feature = "wifi")]
fn json_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 8);
    for ch in input.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(feature = "wifi")]
async fn mqtt_publish_with_discovery(
    stack: Stack<'static>,
    host: &str,
    port_raw: &str,
    username: &str,
    password: &str,
    base_topic_raw: &str,
    device_id_raw: &str,
    wifi_location: &str,
    temp: f32,
    hum: f32,
) {
    let host = host.trim();
    if host.is_empty() {
        return;
    }

    // This firmware uses normal DNS A records. mDNS (.local) is not supported.
    if host.ends_with(".local") {
        log_runtime_line("MQTT: .local host not supported (mDNS). Use broker IP address.");
        return;
    }

    let port = parse_mqtt_port(port_raw);
    let base_topic = base_topic_raw.trim();
    let base_topic = if base_topic.is_empty() {
        "climate_monitor"
    } else {
        base_topic
    };

    let device_id = mqtt_device_id(device_id_raw, wifi_location);
    let client_id = format!("{}-{}", base_topic, device_id);

    let target_ip = match resolve_host_to_ip(stack, host).await {
        Some(ip) => ip,
        None => {
            log_runtime_line("MQTT: DNS/host resolution failed");
            return;
        }
    };

    let mut rx_buf = [0u8; 1024];
    let mut tx_buf = [0u8; 1024];
    let mut socket = TcpSocket::new(stack, &mut rx_buf, &mut tx_buf);

    match embassy_time::with_timeout(
        Duration::from_secs(MQTT_CONNECT_TIMEOUT_SECS),
        socket.connect(IpEndpoint::new(target_ip, port)),
    )
    .await
    {
        Ok(Ok(())) => {}
        _ => {
            log_runtime_line("MQTT: connect timeout/failed");
            socket.abort();
            return;
        }
    }

    match embassy_time::with_timeout(
        Duration::from_secs(MQTT_IO_TIMEOUT_SECS),
        mqtt_connect(&mut socket, client_id.as_str(), username, password),
    )
    .await
    {
        Ok(Ok(())) => {}
        _ => {
            log_runtime_line("MQTT: CONNECT timeout/failed");
            socket.abort();
            return;
        }
    }

    let availability_topic = format!("{}/{}/availability", base_topic, device_id);
    let temp_topic = format!("{}/{}/temperature", base_topic, device_id);
    let hum_topic = format!("{}/{}/humidity", base_topic, device_id);

    let device_name = if wifi_location.trim().is_empty() {
        format!("Climate Monitor {}", device_id)
    } else {
        format!("Climate Monitor ({})", wifi_location.trim())
    };

    // HA discovery topics (retained)
    let temp_config_topic = format!(
        "{}/sensor/{}_temperature/config",
        HA_DISCOVERY_PREFIX, device_id
    );
    let hum_config_topic = format!(
        "{}/sensor/{}_humidity/config",
        HA_DISCOVERY_PREFIX, device_id
    );

    let temp_unique_id = format!("{}_{}_temperature", base_topic, device_id);
    let hum_unique_id = format!("{}_{}_humidity", base_topic, device_id);

    let device_block = format!(
        r#"{{"identifiers":["{}:{}"],"name":"{}","model":"ESP32-C3","manufacturer":"Climate Monitor"}}"#,
        base_topic,
        device_id,
        json_escape(device_name.as_str())
    );

    let temp_discovery_payload = format!(
        r#"{{"name":"Temperature","unique_id":"{}","state_topic":"{}","unit_of_measurement":"°C","device_class":"temperature","state_class":"measurement","availability_topic":"{}","payload_available":"online","payload_not_available":"offline","device":{}}}"#,
        json_escape(temp_unique_id.as_str()),
        json_escape(temp_topic.as_str()),
        json_escape(availability_topic.as_str()),
        device_block,
    );
    let hum_discovery_payload = format!(
        r#"{{"name":"Humidity","unique_id":"{}","state_topic":"{}","unit_of_measurement":"%","device_class":"humidity","state_class":"measurement","availability_topic":"{}","payload_available":"online","payload_not_available":"offline","device":{}}}"#,
        json_escape(hum_unique_id.as_str()),
        json_escape(hum_topic.as_str()),
        json_escape(availability_topic.as_str()),
        device_block,
    );

    let _ = embassy_time::with_timeout(
        Duration::from_secs(MQTT_IO_TIMEOUT_SECS),
        mqtt_publish(
            &mut socket,
            temp_config_topic.as_str(),
            temp_discovery_payload.as_bytes(),
            true,
        ),
    )
    .await;
    let _ = embassy_time::with_timeout(
        Duration::from_secs(MQTT_IO_TIMEOUT_SECS),
        mqtt_publish(
            &mut socket,
            hum_config_topic.as_str(),
            hum_discovery_payload.as_bytes(),
            true,
        ),
    )
    .await;

    // Availability + readings (retain to keep last values)
    let _ = embassy_time::with_timeout(
        Duration::from_secs(MQTT_IO_TIMEOUT_SECS),
        mqtt_publish(&mut socket, availability_topic.as_str(), b"online", true),
    )
    .await;
    let temp_payload = format!("{:.2}", temp);
    let hum_payload = format!("{:.2}", hum);
    let _ = embassy_time::with_timeout(
        Duration::from_secs(MQTT_IO_TIMEOUT_SECS),
        mqtt_publish(&mut socket, temp_topic.as_str(), temp_payload.as_bytes(), true),
    )
    .await;
    let _ = embassy_time::with_timeout(
        Duration::from_secs(MQTT_IO_TIMEOUT_SECS),
        mqtt_publish(&mut socket, hum_topic.as_str(), hum_payload.as_bytes(), true),
    )
    .await;

    let _ = embassy_time::with_timeout(
        Duration::from_secs(MQTT_IO_TIMEOUT_SECS),
        mqtt_disconnect(&mut socket),
    )
    .await;
    socket.close();
    let _ = embassy_time::with_timeout(Duration::from_secs(MQTT_IO_TIMEOUT_SECS), socket.flush()).await;
    log_runtime_line("MQTT: publish cycle done");
}

#[cfg(feature = "wifi")]
async fn mqtt_connect(
    socket: &mut TcpSocket<'_>,
    client_id: &str,
    username: &str,
    password: &str,
) -> Result<(), ()> {
    let username = username.trim();
    let password = password.trim();
    let has_user = !username.is_empty();
    let has_pass = has_user && !password.is_empty();

    if !has_user && !password.is_empty() {
        log_runtime_line("MQTT: password provided without username; ignoring password");
    }

    let mut payload: Vec<u8> = Vec::new();
    push_mqtt_utf8(&mut payload, client_id);
    if has_user {
        push_mqtt_utf8(&mut payload, username);
    }
    if has_pass {
        push_mqtt_utf8(&mut payload, password);
    }

    let mut vh: [u8; 10] = [0; 10];
    vh[0] = 0;
    vh[1] = 4;
    vh[2] = b'M';
    vh[3] = b'Q';
    vh[4] = b'T';
    vh[5] = b'T';
    vh[6] = 4; // 3.1.1

    let mut flags = 0x02u8; // Clean session
    if has_user {
        flags |= 0x80;
    }
    if has_pass {
        flags |= 0x40;
    }
    vh[7] = flags;
    vh[8] = 0;
    vh[9] = 60; // keepalive

    let remaining_len = vh.len() + payload.len();
    let mut packet: Vec<u8> = Vec::with_capacity(2 + 4 + remaining_len);
    packet.push(0x10);
    encode_remaining_length(&mut packet, remaining_len);
    packet.extend_from_slice(&vh);
    packet.extend_from_slice(&payload);

    socket_write_all(socket, &packet).await.map_err(|_| ())?;

    let mut connack = [0u8; 4];
    socket_read_exact(socket, &mut connack).await.map_err(|_| ())?;
    if connack[0] != 0x20 || connack[1] != 0x02 || connack[3] != 0x00 {
        return Err(());
    }

    Ok(())
}

#[cfg(feature = "wifi")]
async fn mqtt_publish(
    socket: &mut TcpSocket<'_>,
    topic: &str,
    payload: &[u8],
    retain: bool,
) -> Result<(), ()> {
    let topic_bytes = topic.as_bytes();
    let mut header: Vec<u8> = Vec::new();
    let fixed = if retain { 0x31u8 } else { 0x30u8 };
    header.push(fixed);

    let remaining_len = 2 + topic_bytes.len() + payload.len();
    encode_remaining_length(&mut header, remaining_len);

    let mut packet: Vec<u8> = Vec::with_capacity(header.len() + remaining_len);
    packet.extend_from_slice(&header);
    packet.push(((topic_bytes.len() >> 8) & 0xFF) as u8);
    packet.push((topic_bytes.len() & 0xFF) as u8);
    packet.extend_from_slice(topic_bytes);
    packet.extend_from_slice(payload);

    socket_write_all(socket, &packet).await.map_err(|_| ())?;
    Ok(())
}

#[cfg(feature = "wifi")]
async fn mqtt_disconnect(socket: &mut TcpSocket<'_>) -> Result<(), ()> {
    socket_write_all(socket, &[0xE0, 0x00]).await.map_err(|_| ())
}

#[cfg(feature = "wifi")]
fn push_mqtt_utf8(out: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    let len = bytes.len().min(u16::MAX as usize);
    out.push(((len >> 8) & 0xFF) as u8);
    out.push((len & 0xFF) as u8);
    out.extend_from_slice(&bytes[..len]);
}

#[cfg(feature = "wifi")]
fn encode_remaining_length(out: &mut Vec<u8>, mut len: usize) {
    loop {
        let mut encoded = (len % 128) as u8;
        len /= 128;
        if len > 0 {
            encoded |= 0x80;
        }
        out.push(encoded);
        if len == 0 {
            break;
        }
    }
}

#[cfg(feature = "wifi")]
async fn socket_write_all(
    socket: &mut TcpSocket<'_>,
    mut data: &[u8],
) -> Result<(), embassy_net::tcp::Error> {
    while !data.is_empty() {
        let n = match embassy_time::with_timeout(
            Duration::from_secs(MQTT_IO_TIMEOUT_SECS),
            socket.write(data),
        )
        .await
        {
            Ok(Ok(n)) => n,
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err(embassy_net::tcp::Error::ConnectionReset),
        };
        if n == 0 {
            return Err(embassy_net::tcp::Error::ConnectionReset);
        }
        data = &data[n..];
    }
    Ok(())
}

#[cfg(feature = "wifi")]
async fn socket_read_exact(
    socket: &mut TcpSocket<'_>,
    mut out: &mut [u8],
) -> Result<(), embassy_net::tcp::Error> {
    while !out.is_empty() {
        let n = match embassy_time::with_timeout(
            Duration::from_secs(MQTT_IO_TIMEOUT_SECS),
            socket.read(out),
        )
        .await
        {
            Ok(Ok(n)) => n,
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err(embassy_net::tcp::Error::ConnectionReset),
        };
        if n == 0 {
            return Err(embassy_net::tcp::Error::ConnectionReset);
        }
        let tmp = out;
        out = &mut tmp[n..];
    }
    Ok(())
}
