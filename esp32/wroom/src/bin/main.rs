#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

extern crate alloc;

use alloc::{boxed::Box, format, string::String, vec::Vec};

use embedded_dht_rs::dht22::Dht22;
use embassy_executor::Spawner;
use embassy_net::{tcp::client::TcpClientState, Stack};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Instant, Timer};
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    delay::Delay,
    gpio::{Input, InputConfig, Level, Pull},
    i2c::master::{Config as I2cConfig, I2c},
    time::Rate,
    timer::timg::TimerGroup,
};
use esp_rtos as _;
use esp_storage::FlashStorage;
use log_utils::set_time_sync_from_unix;
use network::verify_dns;
use post_readings::post_readings;
use ssd1306::{I2CDisplayInterface, Ssd1306, prelude::*};
use static_cell::StaticCell;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

mod body;
mod dht22;
mod log_utils;
mod network;
mod post_readings;
mod server_time_response;
mod time_utils;
mod wifi;

#[path = "main/app_config.rs"]
mod app_config;
#[path = "main/config_portal.rs"]
mod config_portal;
#[path = "main/wifi_runtime.rs"]
mod wifi_runtime;
#[path = "../oled_display.rs"]
mod oled_display;

use app_config::{AppConfig, FlashConfigStorage, resolve_partition_range};
use wifi_runtime::WifiCommand;

esp_bootloader_esp_idf::esp_app_desc!();

const AP_SSID_PREFIX: &str = "ClimateMonitor-";
const DEFAULT_SENSOR_INTERVAL_SECS: u64 = 30;
const MIN_SENSOR_INTERVAL_SECS: u64 = 5;
const MAX_SENSOR_INTERVAL_SECS: u64 = 3600;
const MAIN_LOOP_TICK_MS: u64 = 250;
const OLED_REFRESH_MS: u64 = 1000;
const PRECONNECT_SCAN_WAIT_MS: u64 = 2500;
const BOOT_PRESCAN_ATTEMPTS: u8 = 4;
const SAVE_AP_CLOSE_DELAY_SECS: u64 = 2;
const MAX_CONSECUTIVE_POST_ERRORS: u8 = 3;
const HEARTBEAT_EVERY_CYCLES: u32 = 10;
const HTTP_CONNECT_TIMEOUT_SECS: u64 = 10;
const HTTP_SEND_TIMEOUT_SECS: u64 = 10;
const POST_TOTAL_TIMEOUT_SECS: u64 = 25;
const OLED_BOOT_INIT_RETRIES: u8 = 5;
const OLED_BOOT_INIT_RETRY_DELAY_MS: u32 = 150;
const BOOT_CONNECT_TIMEOUT_SECS: u64 = 15;
const BUTTON_EVENT_NONE: u8 = 0;
const BUTTON_EVENT_SHORT: u8 = 1;
const BUTTON_EVENT_LONG: u8 = 2;

static NET_STACK: StaticCell<Stack<'static>> = StaticCell::new();
static TCP_STATE: StaticCell<TcpClientState<1, 4096, 4096>> = StaticCell::new();
static BUTTON_PRESSED: AtomicBool = AtomicBool::new(false);
static BUTTON_EVENT: AtomicU8 = AtomicU8::new(BUTTON_EVENT_NONE);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ButtonEvent {
    ShortPress,
    LongPress,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PortalMode {
    Off,
    MissingConfig,
    Manual,
    Recovery,
}

impl PortalMode {
    fn is_open(self) -> bool {
        !matches!(self, Self::Off)
    }
}

fn log_runtime_line(line: &str) {
    logln!("{}", line);
    wifi_runtime::portal_log(line);
}

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

#[embassy_executor::task]
async fn gpio_poll_task(btn: Input<'static>) {
    loop {
        Timer::after(Duration::from_millis(10)).await;
        BUTTON_PRESSED.store(btn.level() == Level::Low, Ordering::Relaxed);
    }
}

#[embassy_executor::task]
async fn button_task() {
    loop {
        while !BUTTON_PRESSED.load(Ordering::Relaxed) {
            Timer::after(Duration::from_millis(20)).await;
        }
        let press_start = Instant::now();

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

fn oled_rotation_from_config(orientation: &str) -> DisplayRotation {
    match orientation {
        "portrait" => DisplayRotation::Rotate90,
        _ => DisplayRotation::Rotate0,
    }
}

fn oled_is_portrait(orientation: &str) -> bool {
    orientation == "portrait"
}

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

fn oled_i2c_address() -> u8 {
    match option_env!("OLED_I2C_ADDR") {
        Some("0x3D") | Some("0X3D") | Some("61") => 0x3D,
        _ => 0x3C,
    }
}

fn has_wifi_config(cfg: &AppConfig) -> bool {
    !cfg.wifi_ssid.is_empty()
        || !cfg.wifi_ssid_2.is_empty()
        || !cfg.wifi_ssid_3.is_empty()
        || !cfg.wifi_ssid_4.is_empty()
        || !cfg.wifi_ssid_5.is_empty()
}

fn sensor_interval_secs(cfg: &AppConfig) -> u64 {
    cfg.sensor_interval_secs
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|secs| (MIN_SENSOR_INTERVAL_SECS..=MAX_SENSOR_INTERVAL_SECS).contains(secs))
        .unwrap_or(DEFAULT_SENSOR_INTERVAL_SECS)
}

fn wifi_display_label(location: &str, ssid: &str) -> String {
    if location.is_empty() {
        ssid.into()
    } else {
        format!("{}/{}", location, ssid)
    }
}

fn find_best_ssid(cfg: &AppConfig) -> Option<(String, String, String)> {
    let slots = [
        (cfg.wifi_ssid.as_str(), cfg.wifi_password.as_str(), cfg.wifi_location.as_str()),
        (cfg.wifi_ssid_2.as_str(), cfg.wifi_password_2.as_str(), cfg.wifi_location_2.as_str()),
        (cfg.wifi_ssid_3.as_str(), cfg.wifi_password_3.as_str(), cfg.wifi_location_3.as_str()),
        (cfg.wifi_ssid_4.as_str(), cfg.wifi_password_4.as_str(), cfg.wifi_location_4.as_str()),
        (cfg.wifi_ssid_5.as_str(), cfg.wifi_password_5.as_str(), cfg.wifi_location_5.as_str()),
    ];

    for (ssid, pass, location) in slots {
        if !ssid.is_empty() && wifi_runtime::scan_ssid_visible(ssid) {
            return Some((String::from(ssid), String::from(pass), String::from(location.trim())));
        }
    }

    slots
        .into_iter()
        .find(|(ssid, _, _)| !ssid.is_empty())
        .map(|(ssid, pass, location)| {
            (String::from(ssid), String::from(pass), String::from(location.trim()))
        })
}

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
            wifi_sender.send(WifiCommand::APUp(AP_SSID_PREFIX.into())).await;
        }
        (true, false) => {
            wifi_sender.send(WifiCommand::APDown).await;
        }
        _ => {}
    }

    *portal_mode = next_mode;
}

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
                format!("WiFi pre-scan: no configured visible SSID matched, using fallback: {}", ssid)
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
        format!(
            "WiFi pre-scan complete: {} visible network(s) across {} attempts.",
            count, BOOT_PRESCAN_ATTEMPTS
        )
        .as_str(),
    );
    log_runtime_line(format!("Visible SSIDs:\n{}", merged_flat).as_str());
}

async fn scan_and_connect_best_configured_wifi(
    wifi_sender: &wifi_runtime::WifiCommandSender,
    cfg: &AppConfig,
    scan_reason: &str,
    connect_context: &str,
) -> Option<(String, String)> {
    refresh_visible_networks(wifi_sender, scan_reason).await;
    connect_best_configured_wifi(wifi_sender, cfg, connect_context).await
}

#[allow(clippy::large_stack_frames, reason = "it's not unusual to allocate larger buffers etc. in main")]
#[esp_rtos::main]
async fn main(spawner: Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    // BOOT button on GPIO0 (active-low)
    let btn = Input::new(peripherals.GPIO0, InputConfig::default().with_pull(Pull::Up));
    let boot_button_held = btn.level() == Level::Low;
    spawner.spawn(gpio_poll_task(btn)).unwrap();
    spawner.spawn(button_task()).unwrap();

    esp_alloc::heap_allocator!(#[unsafe(link_section = ".dram2_uninit")] size: 64 * 1024);
    esp_alloc::heap_allocator!(size: 36 * 1024);
    esp_println::logger::init_logger_from_env();
    let delay = Delay::new();
    delay.delay_millis(OLED_BOOT_INIT_RETRY_DELAY_MS);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);

    let oled_addr = oled_i2c_address();
    let mut oled = match I2c::new(
        peripherals.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(100)),
    ) {
        Ok(i2c) => {
            let mut i2c = i2c.with_sda(peripherals.GPIO21).with_scl(peripherals.GPIO22);
            let mut found_any = false;
            log_runtime_line("I2C scan start (GPIO21 SDA, GPIO22 SCL)...");
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
            log_runtime_line("OLED: disabling display after repeated init failures");
            oled = None;
        }
    } else {
        log_runtime_line("OLED: disabled (bus init failed)");
    }

    let mut flash = FlashStorage::new(peripherals.FLASH);
    let partition_range = resolve_partition_range(&mut flash, "config").expect("config partition missing");
    let mut config_storage = FlashConfigStorage::new(flash, partition_range);
    config_storage.mount().await.expect("config mount failed");
    let loaded_config = config_storage.load_config().await.expect("config load failed");
    let initial_oled_orientation = loaded_config.oled_orientation.clone();
    let config_handle: &'static Mutex<CriticalSectionRawMutex, AppConfig> =
        Box::leak(Box::new(Mutex::new(loaded_config)));
    let config_storage: &'static Mutex<CriticalSectionRawMutex, FlashConfigStorage<'static>> =
        Box::leak(Box::new(Mutex::new(config_storage)));

    let mut oled_portrait_mode = oled_is_portrait(initial_oled_orientation.as_str());
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

    let stack: &'static Stack<'static> = NET_STACK.init(wifi_stacks.sta);
    let tcp_state: &'static TcpClientState<1, 4096, 4096> = TCP_STATE.init(TcpClientState::new());

    let mut portal_mode = PortalMode::Off;
    let mut active_wifi_location = String::new();
    let mut active_wifi_ssid = String::new();
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

    if portal_mode == PortalMode::Off {
        let cfg = config_handle.lock().await;
        log_runtime_line(format!("Configured API URL: {}", cfg.api_url).as_str());
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
                    draw_status_for_mode(display, format!("Connecting {}", ssid).as_str(), oled_portrait_mode);
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

    if portal_mode == PortalMode::Off {
        log_runtime_line("Boot: waiting up to 15 s for STA connection...");
        let boot_connected = embassy_time::with_timeout(Duration::from_secs(BOOT_CONNECT_TIMEOUT_SECS), stack.wait_config_up())
            .await
            .is_ok();

        if boot_connected {
            log_runtime_line("WiFi connected during boot.");
            if let Some(cfg) = stack.config_v4() {
                log_runtime_line(format!("Got IP: {:?}", cfg.address).as_str());
            }
            let api_url = config_handle.lock().await.api_url.clone();
            if !api_url.is_empty() && !verify_dns(stack, api_url.as_str(), Some(true)).await {
                log_runtime_line("DNS verification failed after boot connect.");
            }
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

    let dht22_pin = dht22::init_dht22_pin(peripherals.GPIO4);
    let mut dht22 = Dht22::new(dht22_pin, Delay::new());
    let mut consecutive_post_errors: u8 = 0;
    let mut last_post_error: Option<alloc::string::String> = None;
    let mut last_sent_secs: usize = 0;
    let mut loop_counter: u32 = 0;
    let mut last_temp: Option<f32> = None;
    let mut last_hum: Option<f32> = None;
    let mut net_was_up = stack.is_config_up();
    let configured_sensor_interval_secs = {
        let cfg = config_handle.lock().await;
        sensor_interval_secs(&cfg)
    };
    let mut next_sensor_at = if trigger_sensor_now_after_boot {
        Instant::now()
    } else {
        Instant::now() + Duration::from_secs(configured_sensor_interval_secs)
    };
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
                        "Button: long press -> AP portal opening: http://192.168.4.1/",
                    )
                    .await;
                }
                ButtonEvent::ShortPress => {
                    if portal_mode.is_open() {
                        let cfg = config_handle.lock().await.clone();
                        if has_wifi_config(&cfg) {
                            set_portal_mode(
                                &wifi_sender,
                                &mut portal_mode,
                                PortalMode::Off,
                                "Button: short press -> AP portal closed",
                            )
                            .await;

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
                            }
                            next_sensor_at = Instant::now();
                        } else {
                            set_portal_mode(
                                &wifi_sender,
                                &mut portal_mode,
                                PortalMode::MissingConfig,
                                "Button: no WiFi config, keeping AP portal open",
                            )
                            .await;
                        }
                    } else {
                        if let Some(display) = oled.as_mut() {
                            draw_status_for_mode(display, "AP: 192.168.4.1", oled_portrait_mode);
                        }
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
                            draw_status_for_mode(display, format!("Connecting {}", ssid).as_str(), oled_portrait_mode);
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
        if now >= next_oled_refresh_at && stack.is_config_up() && !portal_mode.is_open() {
            let total_secs = (now.as_millis() / 1000) as usize;
            let data_sent = total_secs.saturating_sub(last_sent_secs) < 5;
            let wifi_label = wifi_display_label(active_wifi_location.as_str(), active_wifi_ssid.as_str());
            let wifi_width = if data_sent { 16 } else { 21 };
            let should_scroll = !oled_portrait_mode && wifi_label.len() > wifi_width;
            let sent_state_changed = data_sent != last_oled_data_sent;
            next_oled_refresh_at = now + Duration::from_millis(OLED_REFRESH_MS);

            if should_scroll || data_sent || sent_state_changed {
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

        if loop_counter % HEARTBEAT_EVERY_CYCLES == 0 {
            log_runtime_line(
                format!("Heartbeat: has_ipv4={}, portal_open={}", stack.config_v4().is_some(), portal_mode.is_open()).as_str(),
            );
        }
        loop_counter = loop_counter.wrapping_add(1);

        if wifi_runtime::scan_in_progress() {
            log_runtime_line("Scan in progress: pausing sensor post this cycle");
            continue;
        }

        if portal_mode.is_open() && !stack.is_config_up() {
            log_runtime_line("AP active: STA down, sensor post paused");
            continue;
        }

        match dht22::read(&mut dht22) {
            Ok(sensor_reading) => {
                let (temp_offset, hum_offset, api_url, api_key) = {
                    let cfg = config_handle.lock().await;
                    let t: f32 = cfg.temp_offset.parse().unwrap_or(0.0);
                    let h: f32 = cfg.hum_offset.parse().unwrap_or(0.0);
                    (t, h, cfg.api_url.clone(), cfg.api_key.clone())
                };

                let temperature = sensor_reading.temperature + temp_offset;
                let humidity = (sensor_reading.humidity + hum_offset).max(0.0);
                last_temp = Some(temperature);
                last_hum = Some(humidity);

                log_runtime_line(
                    format!(
                        "DHT22 Sensor - Temperature: {:.1} °C, humidity: {:.1} %",
                        temperature, humidity
                    )
                    .as_str(),
                );

                if let Some(display) = oled.as_mut() {
                    if !portal_mode.is_open() {
                        let total_secs = (Instant::now().as_millis() / 1000) as usize;
                        let hours = ((total_secs / 3600) % 24) as u8;
                        let minutes = ((total_secs % 3600) / 60) as u8;
                        let data_sent = total_secs.saturating_sub(last_sent_secs) < 5;
                        let _ = oled_display::show_reading(
                            display,
                            temperature,
                            humidity,
                            hours,
                            minutes,
                            data_sent,
                        );
                        last_oled_data_sent = data_sent;
                    }
                }

                let location = if active_wifi_location.is_empty() {
                    None
                } else {
                    Some(active_wifi_location.as_str())
                };
                let values = [
                    body::SensorValue {
                        sensor_type: "temperature",
                        value: temperature,
                        unit: "C",
                        location,
                    },
                    body::SensorValue {
                        sensor_type: "humidity",
                        value: humidity,
                        unit: "%",
                        location,
                    },
                ];
                let body = body::build_body(&values);
                let wifi_connected = Some(stack.is_config_up());

                let (post_ok, server_unix_sec, post_error) = match embassy_time::with_timeout(
                    Duration::from_secs(POST_TOTAL_TIMEOUT_SECS),
                    post_readings(
                        stack,
                        tcp_state,
                        api_url.as_str(),
                        api_key.as_str(),
                        body.as_bytes(),
                        wifi_connected,
                    ),
                )
                .await
                {
                    Ok(result) => result,
                    Err(_) => {
                        let msg = alloc::format!("POST total timeout after {}s", POST_TOTAL_TIMEOUT_SECS);
                        log_runtime_line(msg.as_str());
                        (false, None, Some(msg))
                    }
                };

                if post_ok {
                    consecutive_post_errors = 0;
                    last_post_error = None;
                    last_sent_secs = (Instant::now().as_millis() / 1000) as usize;
                    if let Some(unix_sec) = server_unix_sec {
                        set_time_sync_from_unix(unix_sec);
                    }
                } else {
                    consecutive_post_errors = consecutive_post_errors.saturating_add(1);
                    if let Some(err) = post_error {
                        last_post_error = Some(err);
                    }
                    log_runtime_line(
                        format!(
                            "POST failure count: {}/{}; last error: {}",
                            consecutive_post_errors,
                            MAX_CONSECUTIVE_POST_ERRORS,
                            last_post_error.as_deref().unwrap_or("unknown")
                        )
                        .as_str(),
                    );
                    if consecutive_post_errors >= MAX_CONSECUTIVE_POST_ERRORS {
                        log_runtime_line("Too many POST failures, restarting...");
                        Delay::new().delay_millis(100);
                        esp_hal::system::software_reset();
                    }
                }
            }
            Err(error) => {
                log_runtime_line(format!("An error occurred while trying to read sensor: {:?}", error).as_str());
            }
        }
    }
}
