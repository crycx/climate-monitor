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
    let mut led = esp_hal::gpio::Output::new(p.GPIO8, esp_hal::gpio::Level::High, esp_hal::gpio::OutputConfig::default());
    let delay = esp_hal::delay::Delay::new();
    loop {
        led.toggle();
        delay.delay_millis(100);
    }
}

#[cfg(feature = "wifi")]
extern crate alloc;

#[cfg(feature = "wifi")]
use alloc::{format, string::String};
#[cfg(feature = "wifi")]
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use embedded_dht_rs::dht22::Dht22;
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    delay::Delay,
    gpio::{DriveMode, Flex, OutputConfig, Pull},
};
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
static BUTTON_EVENT: AtomicU8 = AtomicU8::new(BUTTON_EVENT_NONE);

#[cfg(feature = "wifi")]
const AP_SSID_PREFIX: &str = "ClimateMonitor-";
#[cfg(feature = "wifi")]
const SENSOR_INTERVAL_SECS: u64 = 30;
#[cfg(feature = "wifi")]
const MAIN_LOOP_TICK_MS: u64 = 250;
#[cfg(feature = "wifi")]
const PRECONNECT_SCAN_WAIT_MS: u64 = 2500;
#[cfg(feature = "wifi")]
const SAVE_AP_CLOSE_DELAY_SECS: u64 = 2;
#[cfg(feature = "wifi")]
const BUTTON_EVENT_NONE: u8 = 0;
#[cfg(feature = "wifi")]
const BUTTON_EVENT_SHORT: u8 = 1;
#[cfg(feature = "wifi")]
const BUTTON_EVENT_LONG: u8 = 2;

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
) -> bool {
    if let Some((ssid, pass)) = find_best_ssid(cfg) {
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
        true
    } else {
        false
    }
}

#[cfg(feature = "wifi")]
async fn refresh_visible_networks(
    wifi_sender: &wifi_runtime::WifiCommandSender,
    reason: &str,
) {
    log_runtime_line(reason);
    wifi_sender.send(WifiCommand::ScanOnce).await;
    Timer::after(Duration::from_millis(PRECONNECT_SCAN_WAIT_MS)).await;

    if let Some(flat) = wifi_runtime::scan_results_flat() {
        let count = flat.lines().filter(|ssid| !ssid.is_empty()).count();
        if count == 0 {
            log_runtime_line("WiFi pre-scan complete: no visible networks found.");
        } else {
            log_runtime_line(
                format!("WiFi pre-scan complete: {} visible network(s).", count).as_str(),
            );
            log_runtime_line(format!("Visible SSIDs:\n{}", flat).as_str());
        }
    } else {
        log_runtime_line("WiFi pre-scan complete: no cached scan results.");
    }
}

#[cfg(feature = "wifi")]
async fn scan_and_connect_best_configured_wifi(
    wifi_sender: &wifi_runtime::WifiCommandSender,
    cfg: &AppConfig,
    scan_reason: &str,
    connect_context: &str,
) -> bool {
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

    esp_alloc::heap_allocator!(size: 36 * 1024);
    esp_println::logger::init_logger_from_env();

    let delay = Delay::new();

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
        if AP_ACTIVE.load(Ordering::Relaxed) {
            // AP portal aktiivne: vilgub aeglaselt 500 ms (aktiiv-LOW: low=sees)
            led.set_low();
            Timer::after(Duration::from_millis(500)).await;
            led.set_high();
            Timer::after(Duration::from_millis(500)).await;
        } else if SENDING.load(Ordering::Relaxed) {
            // Andmete saatmine: LED püsivalt sees
            led.set_low();
            Timer::after(Duration::from_millis(50)).await;
        } else {
            // Ooteseisund: LED väljas
            led.set_high();
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
fn find_best_ssid(cfg: &AppConfig) -> Option<(String, String)> {
    let slots = [
        (cfg.wifi_ssid.as_str(), cfg.wifi_password.as_str()),
        (cfg.wifi_ssid_2.as_str(), cfg.wifi_password_2.as_str()),
        (cfg.wifi_ssid_3.as_str(), cfg.wifi_password_3.as_str()),
        (cfg.wifi_ssid_4.as_str(), cfg.wifi_password_4.as_str()),
        (cfg.wifi_ssid_5.as_str(), cfg.wifi_password_5.as_str()),
    ];
    // Prefer a network that is visible in the latest scan results.
    for (ssid, pass) in slots {
        if !ssid.is_empty() && wifi_runtime::scan_ssid_visible(ssid) {
            return Some((String::from(ssid), String::from(pass)));
        }
    }
    // Fall back to the first configured slot.
    slots
        .into_iter()
        .find(|(ssid, _)| !ssid.is_empty())
        .map(|(ssid, pass)| (String::from(ssid), String::from(pass)))
}

#[cfg(feature = "wifi")]
#[esp_rtos::main]
async fn main(spawner: Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(size: 72 * 1024);
    esp_println::logger::init_logger_from_env();

    // Start the RISC-V RTOS scheduler (required by esp_radio::init).
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_ints = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_ints.software_interrupt0);

    // GPIO9 = BOOT button (active-low).
    let btn = Input::new(peripherals.GPIO9, InputConfig::default().with_pull(Pull::Up));
    let boot_button_held = btn.level() == Level::Low;
    spawner.spawn(gpio_poll_task(btn)).unwrap();

    // GPIO8 = built-in LED, vilgub kui AP portal on aktiivne.
    let led = Output::new(peripherals.GPIO8, Level::High, OutputConfig::default());
    spawner.spawn(led_task(led)).unwrap();

    let mut flash = FlashStorage::new(peripherals.FLASH);
    let partition_range = resolve_partition_range(&mut flash, "config").expect("config partition missing");
    let mut config_storage = FlashConfigStorage::new(flash, partition_range);
    config_storage.mount().await.expect("config mount failed");
    let loaded_config = config_storage.load_config().await.expect("config load failed");
    let config_handle: &'static Mutex<CriticalSectionRawMutex, AppConfig> =
        Box::leak(Box::new(Mutex::new(loaded_config)));
    let config_storage: &'static Mutex<CriticalSectionRawMutex, FlashConfigStorage<'static>> =
        Box::leak(Box::new(Mutex::new(config_storage)));

    let (wifi_stacks, wifi_sender) = wifi_runtime::init(&spawner, peripherals.WIFI).await;
    config_portal::start(spawner, wifi_stacks.ap, config_handle, config_storage);
    let config_rx = config_portal::config_update_receiver();

    spawner.spawn(button_task()).unwrap();

    let stack: &'static Stack<'static> = NET_STACK.init(wifi_stacks.sta);
    let tcp_state: &'static TcpClientState<2, 4096, 4096> = TCP_STATE.init(TcpClientState::new());
    let mut portal_mode = PortalMode::Off;

    if boot_button_held {
        wifi_sender.send(WifiCommand::StaDown).await;
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
            scan_and_connect_best_configured_wifi(
                &wifi_sender,
                &cfg,
                "Boot: scanning visible WiFi networks before connect...",
                "Boot: connecting to ",
            )
            .await;
        } else {
            wifi_sender.send(WifiCommand::StaDown).await;
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
        let boot_connected = embassy_time::with_timeout(
            Duration::from_secs(BOOT_CONNECT_TIMEOUT_SECS),
            stack.wait_config_up(),
        )
        .await
        .is_ok();

        if boot_connected {
            log_runtime_line("WiFi connected during boot.");
            if let Some(cfg) = stack.config_v4() {
                log_runtime_line(format!("Got IP: {:?}", cfg.address).as_str());
            }
        } else {
            wifi_sender.send(WifiCommand::StaDown).await;
            set_portal_mode(
                &wifi_sender,
                &mut portal_mode,
                PortalMode::Recovery,
                "Boot: STA connect timeout, starting AP portal at 192.168.4.1",
            )
            .await;
        }
    }

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

    let mut net_was_up = stack.is_config_up();
    let mut next_sensor_at = Instant::now() + Duration::from_secs(SENSOR_INTERVAL_SECS);
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
                    set_portal_mode(
                        &wifi_sender,
                        &mut portal_mode,
                        PortalMode::Manual,
                        "Button: long press -> AP portal opening: http://192.168.4.1/",
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

                        if stack.is_config_up() {
                            log_runtime_line("Button: STA already connected, skipping reconnect.");
                        } else {
                            let cfg = config_handle.lock().await;
                            if has_wifi_config(&cfg) {
                                scan_and_connect_best_configured_wifi(
                                    &wifi_sender,
                                    &cfg,
                                    "Button: scanning visible WiFi networks before reconnect...",
                                    "Button: reconnecting to ",
                                )
                                .await;
                            }
                        }
                    } else {
                        set_portal_mode(
                            &wifi_sender,
                            &mut portal_mode,
                            PortalMode::Manual,
                            "Button: short press -> AP portal opening: http://192.168.4.1/",
                        )
                        .await;
                    }
                }
            }
        }

        let net_is_up_now = stack.is_config_up();
        if net_is_up_now && !net_was_up {
            log_runtime_line("WiFi connected");
            if let Some(cfg) = stack.config_v4() {
                log_runtime_line(format!("Got IP: {:?}", cfg.address).as_str());
            }
        } else if !net_is_up_now && net_was_up {
            log_runtime_line("WiFi disconnected");
        }
        net_was_up = net_is_up_now;

        if let Some(changed) = config_save_changed {
            let has_config = {
                let cfg = config_handle.lock().await;
                if has_wifi_config(&cfg) {
                    let should_reconnect = changed || !stack.is_config_up();
                    if should_reconnect {
                        scan_and_connect_best_configured_wifi(
                            &wifi_sender,
                            &cfg,
                            "Config saved: scanning visible WiFi networks before connect...",
                            "Config saved: connecting to ",
                        )
                        .await;
                    } else {
                        log_runtime_line("Config saved: WiFi already connected, skipping reconnect.");
                    }
                    true
                } else {
                    false
                }
            };

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
                set_portal_mode(
                    &wifi_sender,
                    &mut portal_mode,
                    PortalMode::MissingConfig,
                    "Config saved without WiFi credentials: AP portal remains available.",
                )
                .await;
            }

            next_sensor_at = Instant::now() + Duration::from_secs(SENSOR_INTERVAL_SECS);
            continue;
        }

        if Instant::now() < next_sensor_at {
            continue;
        }
        next_sensor_at = Instant::now() + Duration::from_secs(SENSOR_INTERVAL_SECS);

        if wifi_runtime::scan_in_progress() {
            log_runtime_line("Scan in progress: pausing sensor post this cycle");
            continue;
        }

        if portal_mode.is_open() && !stack.is_config_up() {
            log_runtime_line("AP active: STA down, sensor post paused");
            continue;
        } else if portal_mode.is_open() && stack.is_config_up() {
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
                log_runtime_line(
                    format!(
                        "DHT22: {:.1} C  {:.1} %  (offsets: temp {:+.1} C, hum {:+.1} %)",
                        temp, hum, temp_offset, hum_offset
                    )
                    .as_str(),
                );
                if stack.is_config_up() {
                    post_readings_with_retry(
                        stack,
                        tcp_state,
                        api_url.as_str(),
                        api_key.as_str(),
                        temp,
                        hum,
                    )
                    .await;
                } else {
                    log_runtime_line("Network down, waiting for reconnect...");
                    let reconnected = embassy_time::with_timeout(
                        Duration::from_secs(60),
                        stack.wait_config_up(),
                    )
                    .await
                    .is_ok();
                    if reconnected {
                        log_runtime_line("Reconnected, waiting 5 s before sending...");
                        Timer::after(Duration::from_secs(5)).await;
                        post_readings_with_retry(
                            stack,
                            tcp_state,
                            api_url.as_str(),
                            api_key.as_str(),
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
    temp: f32,
    hum: f32,
) {
    SENDING.store(true, Ordering::Relaxed);
    for attempt in 1..=3u8 {
        if attempt > 1 {
            log_runtime_line(format!("Retry {}/3 in 10 s...", attempt).as_str());
            Timer::after(Duration::from_secs(10)).await;
        }
        if post_readings(stack, tcp_state, api_url, api_key, temp, hum).await {
            SENDING.store(false, Ordering::Relaxed);
            return;
        }
    }
    SENDING.store(false, Ordering::Relaxed);
    log_runtime_line("All 3 POST attempts failed, will retry next cycle.");
}

/// Send temperature + humidity as HTTPS JSON POST to the backend API.
/// Returns true on success.
#[cfg(feature = "wifi")]
async fn post_readings(
    stack: &'static Stack<'static>,
    tcp_state: &'static TcpClientState<2, 4096, 4096>,
    api_url: &str,
    api_key: &str,
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

    let body = format!(
        r#"[{{"sensor_type":"temperature","value":{:.2},"unit":"C"}},{{"sensor_type":"humidity","value":{:.2},"unit":"%"}}]"#,
        temp, hum
    );
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
