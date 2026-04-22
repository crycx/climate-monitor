#![no_std]
#![no_main]
#![recursion_limit = "256"]
#![cfg_attr(feature = "wifi", feature(impl_trait_in_assoc_type))]
#![allow(unexpected_cfgs)]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

/// Panic handler: prindib vea consolesse ja vilgub GPIO8 LED-i kiiresti (100 ms).
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    esp_println::println!("\n\n*** PANIC ***\n{}", info);

    // Steal peripherals — normaalne omandus puudub paanika ajal.
    let p = unsafe { esp_hal::peripherals::Peripherals::steal() };
    let mut led = esp_hal::gpio::Output::new(p.GPIO8, led_level(false), esp_hal::gpio::OutputConfig::default());
    let delay = esp_hal::delay::Delay::new();
    loop {
        led.toggle();
        delay.delay_millis(100);
    }
}

#[cfg(feature = "wifi")]
extern crate alloc;

#[cfg(feature = "wifi")]
use alloc::{format, string::String, vec::Vec};
#[cfg(feature = "wifi")]
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

use embedded_dht_rs::dht22::Dht22;
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    delay::Delay,
    gpio::{DriveMode, Flex, OutputConfig, Pull},
    i2c::master::{Config as I2cConfig, I2c},
    time::Rate,
};
#[cfg(not(feature = "wifi"))]
use esp_hal::time::Instant as HalInstant;
use ssd1306::{I2CDisplayInterface, Ssd1306, prelude::*};
#[cfg(feature = "wifi")]
use esp_hal::gpio::{Input, InputConfig, Level, Output};

// ── WiFi-only imports ────────────────────────────────────────────────────────
#[cfg(feature = "wifi")]
use embassy_executor::Spawner;
#[cfg(feature = "wifi")]
use embassy_net::{Stack, dns::DnsSocket, tcp::client::TcpClientState};
#[cfg(feature = "wifi")]
use embassy_time::{Duration, Instant, Timer};
#[cfg(feature = "wifi")]
use esp_hal::interrupt::software::SoftwareInterruptControl;
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
use esp_radio::wifi::WifiStaState;
#[cfg(feature = "wifi")]
use static_cell::StaticCell;

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
static LAST_SENT_SECS: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "wifi")]
static BUTTON_EVENT: AtomicU8 = AtomicU8::new(BUTTON_EVENT_NONE);

#[cfg(feature = "wifi")]
const AP_SSID_PREFIX: &str = "ClimateMonitor-";
#[cfg(feature = "wifi")]
const DEFAULT_SENSOR_INTERVAL_SECS: u64 = 30;
#[cfg(feature = "wifi")]
const MIN_SENSOR_INTERVAL_SECS: u64 = 5;
#[cfg(feature = "wifi")]
const MAX_SENSOR_INTERVAL_SECS: u64 = 3600;
#[cfg(feature = "wifi")]
const MAIN_LOOP_TICK_MS: u64 = 250;
#[cfg(feature = "wifi")]
const OLED_REFRESH_MS: u64 = 1000;
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

fn led_active_high() -> bool {
    cfg!(feature = "supermini")
        || matches!(
            option_env!("LED_ACTIVE_HIGH"),
            Some("1" | "true" | "TRUE" | "yes" | "YES" | "high" | "HIGH")
        )
}

fn led_level(is_on: bool) -> esp_hal::gpio::Level {
    match (is_on, led_active_high()) {
        (true, true) => esp_hal::gpio::Level::High,
        (true, false) => esp_hal::gpio::Level::Low,
        (false, true) => esp_hal::gpio::Level::Low,
        (false, false) => esp_hal::gpio::Level::High,
    }
}

fn oled_i2c_address() -> u8 {
    match option_env!("OLED_I2C_ADDR") {
        Some("0x3D") | Some("0X3D") | Some("61") => 0x3D,
        _ => 0x3C,
    }
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
fn has_wifi_config(cfg: &AppConfig) -> bool {
    !cfg.wifi_ssid.is_empty()
        || !cfg.wifi_ssid_2.is_empty()
        || !cfg.wifi_ssid_3.is_empty()
        || !cfg.wifi_ssid_4.is_empty()
        || !cfg.wifi_ssid_5.is_empty()
}

#[cfg(feature = "wifi")]
fn sensor_interval_secs(cfg: &AppConfig) -> u64 {
    cfg.sensor_interval_secs
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|secs| (MIN_SENSOR_INTERVAL_SECS..=MAX_SENSOR_INTERVAL_SECS).contains(secs))
        .unwrap_or(DEFAULT_SENSOR_INTERVAL_SECS)
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
fn drive_led(led: &mut Output<'static>, is_on: bool) {
    match led_level(is_on) {
        Level::High => led.set_high(),
        Level::Low => led.set_low(),
    }
}

#[cfg(feature = "wifi")]
fn stack_has_ipv4(stack: &Stack<'static>) -> bool {
    stack.is_config_up() || stack.config_v4().is_some()
}

#[cfg(feature = "wifi")]
fn wifi_sta_connected() -> bool {
    esp_radio::wifi::sta_state() == WifiStaState::Connected
}

#[cfg(feature = "wifi")]
async fn wait_for_sta_network(stack: &'static Stack<'static>, timeout: Duration) -> bool {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if stack_has_ipv4(stack) {
            return true;
        }
        Timer::after(Duration::from_millis(250)).await;
    }
    stack_has_ipv4(stack)
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
async fn connect_best_configured_wifi(
    wifi_sender: &wifi_runtime::WifiCommandSender,
    cfg: &AppConfig,
    context: &str,
) -> Option<(String, String)> {
    if let Some((ssid, pass, location)) = find_best_ssid(cfg) {
        let selected_ssid = ssid.clone();
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
        Some((selected_ssid, location))
    } else {
        None
    }
}

#[cfg(feature = "wifi")]
async fn refresh_visible_networks(
    wifi_sender: &wifi_runtime::WifiCommandSender,
    reason: &str,
) {
    log_runtime_line(reason);
    let mut merged_ssids: Vec<String> = Vec::new();

    for attempt in 1..=BOOT_PRESCAN_ATTEMPTS {
        log_runtime_line(format!("WiFi pre-scan attempt {}/{}...", attempt, BOOT_PRESCAN_ATTEMPTS).as_str());
        wifi_sender.send(WifiCommand::ScanOnce).await;
        Timer::after(Duration::from_millis(PRECONNECT_SCAN_WAIT_MS)).await;

        if let Some(flat) = wifi_runtime::scan_results_flat() {
            for ssid in flat.lines().filter(|ssid| !ssid.is_empty()) {
                if !merged_ssids.iter().any(|existing| existing.as_str() == ssid) {
                    merged_ssids.push(String::from(ssid));
                }
            }
        }
    }

    if merged_ssids.is_empty() {
        log_runtime_line("WiFi pre-scan complete: no visible networks found.");
        return;
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
}

#[cfg(feature = "wifi")]
async fn scan_and_connect_best_configured_wifi(
    wifi_sender: &wifi_runtime::WifiCommandSender,
    cfg: &AppConfig,
    scan_reason: &str,
    connect_context: &str,
) -> Option<(String, String)> {
    refresh_visible_networks(wifi_sender, scan_reason).await;
    connect_best_configured_wifi(wifi_sender, cfg, connect_context).await
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
#[embassy_executor::task]
async fn led_task(mut led: Output<'static>) {
    loop {
        if BOOTING.load(Ordering::Relaxed) {
            // Boot in progress: keep LED ON until WiFi is connected or AP mode is entered.
            drive_led(&mut led, true);
            Timer::after(Duration::from_millis(100)).await;
        } else if AP_ACTIVE.load(Ordering::Relaxed) {
            // AP portal aktiivne: vilgub aeglaselt; polarity is controlled by LED_ACTIVE_HIGH.
            drive_led(&mut led, true);
            Timer::after(Duration::from_millis(500)).await;
            drive_led(&mut led, false);
            Timer::after(Duration::from_millis(500)).await;
        } else if SENDING.load(Ordering::Relaxed) {
            // Andmete saatmine: LED püsivalt sees
            drive_led(&mut led, true);
            Timer::after(Duration::from_millis(50)).await;
        } else {
            // Ooteseisund: LED väljas
            drive_led(&mut led, false);
            Timer::after(Duration::from_millis(100)).await;
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

/// Selects the best WiFi credential to use.
///
/// If scan results are available, prefers the first configured slot whose SSID is
/// currently visible over-the-air.
/// Falls back to the first non-empty slot when no scan data is present.
#[cfg(feature = "wifi")]
fn find_best_ssid(cfg: &AppConfig) -> Option<(String, String, String)> {
    let slots = [
        (cfg.wifi_ssid.as_str(), cfg.wifi_password.as_str(), cfg.wifi_location.as_str()),
        (cfg.wifi_ssid_2.as_str(), cfg.wifi_password_2.as_str(), cfg.wifi_location_2.as_str()),
        (cfg.wifi_ssid_3.as_str(), cfg.wifi_password_3.as_str(), cfg.wifi_location_3.as_str()),
        (cfg.wifi_ssid_4.as_str(), cfg.wifi_password_4.as_str(), cfg.wifi_location_4.as_str()),
        (cfg.wifi_ssid_5.as_str(), cfg.wifi_password_5.as_str(), cfg.wifi_location_5.as_str()),
    ];
    // Prefer a network that is visible in the latest scan results.
    for (ssid, pass, location) in slots {
        if !ssid.is_empty() && wifi_runtime::scan_ssid_visible(ssid) {
            return Some((
                String::from(ssid),
                String::from(pass),
                String::from(location.trim()),
            ));
        }
    }
    // Fall back to the first configured slot.
    slots
        .into_iter()
        .find(|(ssid, _, _)| !ssid.is_empty())
        .map(|(ssid, pass, location)| {
            (
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

    // GPIO9 = BOOT button (active-low).
    let btn = Input::new(peripherals.GPIO9, InputConfig::default().with_pull(Pull::Up));
    let boot_button_held = btn.level() == Level::Low;
    spawner.spawn(gpio_poll_task(btn)).unwrap();

    // GPIO8 = built-in LED. Set LED_ACTIVE_HIGH=1 for boards like the C3 Super Mini.
    let led = Output::new(peripherals.GPIO8, led_level(false), OutputConfig::default());
    spawner.spawn(led_task(led)).unwrap();

    let mut flash = FlashStorage::new(peripherals.FLASH);

    let oled_addr = oled_i2c_address();
    let mut oled = match I2c::new(
        peripherals.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(400)),
    ) {
        Ok(i2c) => {
            let mut i2c = i2c.with_sda(peripherals.GPIO6).with_scl(peripherals.GPIO7);
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

    let (wifi_stacks, wifi_sender) = wifi_runtime::init(&spawner, peripherals.WIFI).await;
    config_portal::start(spawner, wifi_stacks.ap, config_handle, config_storage);
    let config_rx = config_portal::config_update_receiver();

    spawner.spawn(button_task()).unwrap();

    let stack: &'static Stack<'static> = NET_STACK.init(wifi_stacks.sta);
    let tcp_state: &'static TcpClientState<2, 4096, 4096> = TCP_STATE.init(TcpClientState::new());
    let mut portal_mode = PortalMode::Off;
    let mut active_wifi_location = String::new();
    let mut active_wifi_ssid = String::new();
    let mut oled_portrait_mode = oled_is_portrait(initial_oled_orientation.as_str());
    let mut trigger_sensor_now_after_boot = false;

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
    }

    // ── Boot: one STA attempt, then AP fallback ─────────────────────────────
    const BOOT_CONNECT_TIMEOUT_SECS: u64 = 15;

    if portal_mode == PortalMode::Off {
    {
        let cfg = config_handle.lock().await;
        if has_wifi_config(&cfg) {
            if let Some(display) = oled.as_mut() {
                draw_status_for_mode(display, "WiFi scan...", oled_portrait_mode);
            }
            if let Some((ssid, location)) = scan_and_connect_best_configured_wifi(
                &wifi_sender,
                &cfg,
                "Boot: scanning visible WiFi networks before connect...",
                "Boot: connecting to ",
            )
            .await {
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
    loop {
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
                        "Button: long press -> AP portal opening (STA kept active): http://192.168.4.1/",
                    )
                    .await;
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
                        } else {
                            let cfg = config_handle.lock().await;
                            if has_wifi_config(&cfg) {
                                if let Some((ssid, location)) = scan_and_connect_best_configured_wifi(
                                    &wifi_sender,
                                    &cfg,
                                    "Button: scanning visible WiFi networks before reconnect...",
                                    "Button: reconnecting to ",
                                )
                                .await {
                                    active_wifi_ssid = ssid.clone();
                                    active_wifi_location = location;
                                    if let Some(display) = oled.as_mut() {
                                        draw_status_for_mode(
                                            display,
                                            format!("Connecting {}", ssid).as_str(),
                                            oled_portrait_mode,
                                        );
                                    }
                                    next_sensor_at = Instant::now();
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
                            "Button: short press -> AP portal opening (STA kept active): http://192.168.4.1/",
                        )
                        .await;
                    }
                }
            }
        }

        let net_is_up_now = stack_has_ipv4(stack);
        if net_is_up_now && !net_was_up {
            log_runtime_line("WiFi connected");
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
                draw_status_for_mode(display, "WiFi disconnected", oled_portrait_mode);
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
                        if let Some((ssid, location)) = scan_and_connect_best_configured_wifi(
                            &wifi_sender,
                            &cfg,
                            "Config saved: scanning visible WiFi networks before connect...",
                            "Config saved: connecting to ",
                        )
                        .await {
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

            // Periodic redraw is only needed for marquee scrolling or transient SENT indicator.
            if should_scroll || data_sent || sent_state_changed {
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
                let (temp_offset, hum_offset, api_url, api_key) = {
                    let cfg = config_handle.lock().await;
                    let t: f32 = cfg.temp_offset.parse().unwrap_or(-1.7);
                    let h: f32 = cfg.hum_offset.parse().unwrap_or(-7.0);
                    (t, h, cfg.api_url.clone(), cfg.api_key.clone())
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
                    }
                }
                if stack_has_ipv4(stack) {
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
                        wifi_sender.send(WifiCommand::StaDown).await;
                        set_portal_mode(
                            &wifi_sender,
                            &mut portal_mode,
                            PortalMode::Recovery,
                            "Reconnect timed out after 60 s, opening AP portal for recovery.",
                        )
                        .await;
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
    #[cfg(not(feature = "no-tls"))]
    let mut tls_rx = [0u8; 4096];
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
