//! Climate Monitor — WiFi provisioning binary
//!
//! Flash this binary once to configure WiFi credentials and API settings
//! via browser. Afterwards flash the main binary which reads config from flash.
//!
//! Boot behaviour:
//!   • If no WiFi credentials saved in flash  → AP portal opens automatically.
//!   • If credentials are present             → connects STA (portal stays off).
//!
//! GPIO9 (BOOT button on ESP32-C3 dev boards, active-low):
//!   • Short press (< 3 s) → toggle AP portal ON / OFF.
//!   • Long press  (≥ 3 s) → force AP portal on (useful to re-enter config).
//!
//! AP network : "ClimateMonitor-XXXX"  (open, no password)
//! Config UI  : http://192.168.2.1
#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]

extern crate alloc;

use alloc::string::String;
use core::sync::atomic::{AtomicBool, Ordering};

use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    gpio::{Input, InputConfig, Level, Pull},
    timer::timg::TimerGroup,
};
use esp_storage::FlashStorage;
use esp_wifi_caddy::{WifiCaddyCommand, WifiCommandSender};
use wifi_caddy_proc::WifiCaddyConfig;

esp_bootloader_esp_idf::esp_app_desc!();

// ── Config struct ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, WifiCaddyConfig)]
#[config_server]
#[config_notify]
#[config_ui(
    page_heading = "Climate Monitor Setup",
    subtitle = "Configure WiFi and API access for this device"
)]
pub struct AppConfig {
    #[config_store(env_default = "WIFI_SSID", notify = "Wifi")]
    #[config_form(fieldset = "WiFi", help = "WiFi network name")]
    wifi_ssid: String,

    #[config_store(env_default = "WIFI_PASSWORD", notify = "Wifi")]
    #[config_form(fieldset = "WiFi", input_type = "password", help = "WiFi network password")]
    wifi_password: String,

    #[config_store(env_default = "API_URL")]
    #[config_form(fieldset = "Backend", help = "Base API URL, e.g. https://climate.sinpal.ee/api/")]
    api_url: String,

    #[config_store(env_default = "API_KEY")]
    #[config_form(fieldset = "Backend", input_type = "password", help = "Device API key")]
    api_key: String,
}

// ── Shared state ──────────────────────────────────────────────────────────────

static BUTTON_PRESSED: AtomicBool = AtomicBool::new(false);
static AP_ACTIVE: AtomicBool = AtomicBool::new(false);

// ── Tasks ─────────────────────────────────────────────────────────────────────

/// Polls GPIO9 every 10 ms and stores level in BUTTON_PRESSED.
/// Keeps the Input borrow inside this task so button_task can await freely.
#[embassy_executor::task]
async fn gpio_poll_task(btn: Input<'static>) {
    loop {
        Timer::after(Duration::from_millis(10)).await;
        BUTTON_PRESSED.store(btn.level() == Level::Low, Ordering::Relaxed);
    }
}

/// Reacts to GPIO9 presses:
///   short press → toggle AP
///   long press  → force AP on (re-provisioning)
#[embassy_executor::task]
async fn button_task(wifi_sender: WifiCommandSender) {
    loop {
        // Wait for press.
        loop {
            Timer::after(Duration::from_millis(20)).await;
            if BUTTON_PRESSED.load(Ordering::Relaxed) {
                break;
            }
        }
        let press_start = Instant::now();

        // Wait for release.
        loop {
            Timer::after(Duration::from_millis(20)).await;
            if !BUTTON_PRESSED.load(Ordering::Relaxed) {
                break;
            }
        }
        let held = press_start.elapsed();

        if held >= Duration::from_secs(3) {
            esp_println::println!("Button: long press → force AP on");
            AP_ACTIVE.store(true, Ordering::Relaxed);
            wifi_sender
                .send(WifiCaddyCommand::APUp("ClimateMonitor-".into()))
                .await;
        } else if AP_ACTIVE.load(Ordering::Relaxed) {
            esp_println::println!("Button: short press → AP off");
            AP_ACTIVE.store(false, Ordering::Relaxed);
            wifi_sender.send(WifiCaddyCommand::APDown).await;
        } else {
            esp_println::println!("Button: short press → AP on");
            AP_ACTIVE.store(true, Ordering::Relaxed);
            wifi_sender
                .send(WifiCaddyCommand::APUp("ClimateMonitor-".into()))
                .await;
        }
    }
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(size: 96 * 1024);
    esp_println::logger::init_logger_from_env();

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_ints = esp_hal::interrupt::software::SoftwareInterruptControl::new(
        peripherals.SW_INTERRUPT,
    );
    esp_rtos::start(timg0.timer0, sw_ints.software_interrupt0);

    // GPIO9 = BOOT button (active-low, internal pull-up).
    let btn = Input::new(peripherals.GPIO9, InputConfig::default().with_pull(Pull::Up));
    spawner.spawn(gpio_poll_task(btn)).unwrap();

    let flash = FlashStorage::new(peripherals.FLASH);

    let (_wifi_stacks, wifi_sender, config_handle, config_rx) =
        esp_wifi_caddy::wifi_init!(AppConfig, spawner, peripherals.WIFI, flash, "config")
            .expect("wifi_init failed");

    spawner.spawn(button_task(wifi_sender.clone())).unwrap();

    // Check for saved credentials.
    let has_credentials = {
        let cfg = config_handle.config().lock().await;
        !cfg.wifi_ssid.is_empty()
    };

    if has_credentials {
        esp_println::println!("Saved config found → connecting STA (press GPIO9 to open portal)");
        let (ssid, pass) = {
            let cfg = config_handle.config().lock().await;
            (cfg.wifi_ssid.clone(), cfg.wifi_password.clone())
        };
        wifi_sender.send(WifiCaddyCommand::StaUp(ssid, pass)).await;
    } else {
        esp_println::println!("No config → opening AP portal at 192.168.2.1");
        AP_ACTIVE.store(true, Ordering::Relaxed);
        wifi_sender
            .send(WifiCaddyCommand::APUp("ClimateMonitor-".into()))
            .await;
    }

    // React to config changes saved via the portal.
    loop {
        let _changed = config_rx.receive().await;
        let (ssid, pass) = {
            let cfg = config_handle.config().lock().await;
            (cfg.wifi_ssid.clone(), cfg.wifi_password.clone())
        };
        if !ssid.is_empty() {
            esp_println::println!("Config saved → connecting STA");
            wifi_sender.send(WifiCaddyCommand::StaUp(ssid, pass)).await;
            if AP_ACTIVE.load(Ordering::Relaxed) {
                esp_println::println!("Closing AP after config save");
                AP_ACTIVE.store(false, Ordering::Relaxed);
                wifi_sender.send(WifiCaddyCommand::APDown).await;
            }
        }
    }
}
