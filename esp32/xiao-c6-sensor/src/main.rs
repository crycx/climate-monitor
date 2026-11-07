//! Battery climate sensor for Seeed XIAO ESP32C6.
//!
//! Normal (battery) cycle, every `interval` seconds:
//! 1. power the DHT22 from a GPIO, read the battery voltage while it settles
//! 2. read the DHT22, then cut its power
//! 3. start the radio and send one ESP-NOW packet to the gateway
//!    (unicast to the remembered gateway, or a channel search on failure)
//! 4. deep sleep
//!
//! Setup, only on a cold boot (power-on / RESET), during a 3 s window with
//! the yellow LED blinking:
//! - attached to a computer over USB → USB console (`console.rs`)
//! - BOOT pressed → WiFi setup portal (`portal.rs`)
//!
//! Pins (XIAO label / GPIO):
//! - D0 / GPIO0  battery divider (2 x 1 MOhm from BAT+, 100 nF to GND)
//! - D2 / GPIO2  DHT22 DATA
//! - D3 / GPIO21 DHT22 VCC (switched)
//! - GPIO3       RF switch enable (low = on), on-board
//! - GPIO9       BOOT button, on-board
//! - GPIO14      antenna select (low = ceramic, high = U.FL), on-board
//! - GPIO15      yellow user LED (low = on), on-board

#![no_std]
#![no_main]

extern crate alloc;

use core::time::Duration as CoreDuration;

use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};
use esp_hal::{
    clock::CpuClock,
    gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull},
    interrupt::software::SoftwareInterruptControl,
    peripherals::{GPIO9, LPWR},
    rtc_cntl::{Rtc, SocResetReason, sleep::TimerWakeupSource},
    timer::timg::TimerGroup,
};
use esp_storage::FlashStorage;

#[path = "../../shared/espnow_packet.rs"]
mod espnow_packet;

mod config;
mod console;
mod form;
mod hw;
mod portal;

use config::ConfigStore;

esp_bootloader_esp_idf::esp_app_desc!();

const BOOT_WINDOW_MS: u64 = 3000;
/// SOF frame changes needed to treat USB as a computer (not a charger).
const USB_HOST_SOF_CHANGES: u8 = 3;
const PANIC_RETRY_SECS: u64 = 60;

fn serial_log_enabled() -> bool {
    !matches!(option_env!("SERIAL_LOG").map(str::trim), Some("0" | "false" | "no" | "off"))
}

#[macro_export]
macro_rules! log_line {
    ($($arg:tt)*) => {
        if $crate::serial_log_enabled() {
            esp_println::println!($($arg)*);
        }
    };
}

// ── Panic: never spin on battery, sleep and retry from a clean boot ─────────

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    esp_println::println!("\n*** PANIC ***\n{}", info);
    // SAFETY: we never return, so nothing else uses LPWR after this.
    let mut rtc = Rtc::new(unsafe { LPWR::steal() });
    rtc.sleep_deep(&[&TimerWakeupSource::new(CoreDuration::from_secs(PANIC_RETRY_SECS))]);
}

// ── Boot mode selection ─────────────────────────────────────────────────────

enum Mode {
    Normal,
    Console,
    Portal,
}

/// Blinks the LED for the boot window and watches the BOOT button and USB.
async fn select_mode(button: GPIO9<'static>, led: &mut Output<'static>) -> Mode {
    let button = Input::new(button, InputConfig::default().with_pull(Pull::Up));
    let mut last_sof = hw::usb_sof_frame();
    let mut sof_changes = 0u8;
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(BOOT_WINDOW_MS) {
        if button.is_low() {
            led.set_low();
            return Mode::Portal;
        }
        let sof = hw::usb_sof_frame();
        if sof != last_sof {
            last_sof = sof;
            sof_changes += 1;
            if sof_changes >= USB_HOST_SOF_CHANGES {
                led.set_low();
                return Mode::Console;
            }
        }
        // Fast blink: 100 ms on / 100 ms off.
        if (started.elapsed().as_millis() / 100) % 2 == 0 {
            led.set_low();
        } else {
            led.set_high();
        }
        Timer::after(Duration::from_millis(10)).await;
    }
    led.set_high();
    Mode::Normal
}

// ── Main ────────────────────────────────────────────────────────────────────

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    esp_alloc::heap_allocator!(size: 72 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_ints = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_ints.software_interrupt0);
    let awake_since = Instant::now();

    let mut store = ConfigStore::new(FlashStorage::new(peripherals.FLASH));
    let (cfg, from_flash) = store.load();
    let mut sensors = hw::Sensors::new(peripherals.GPIO21, peripherals.GPIO2, peripherals.ADC1, peripherals.GPIO0);
    let rf = hw::rf_switch(peripherals.GPIO3, peripherals.GPIO14, cfg.external_antenna);

    // Setup modes are only offered after power-on / RESET, never on a timer wake.
    if esp_hal::system::reset_reason() != Some(SocResetReason::CoreDeepSleep) {
        let mut led = Output::new(peripherals.GPIO15, Level::High, OutputConfig::default());
        log_line!(
            "Boot: settings from {}; press BOOT within 3 s for the WiFi portal",
            if from_flash { "flash" } else { "build defaults" }
        );
        match select_mode(peripherals.GPIO9, &mut led).await {
            Mode::Console => {
                let radio = hw::Radio::start(peripherals.WIFI, rf);
                console::run(cfg, store, sensors, radio, peripherals.USB_DEVICE, peripherals.LPWR).await
            }
            Mode::Portal => portal::run(spawner, cfg, store, sensors, peripherals.WIFI, rf, led).await,
            Mode::Normal => {}
        }
    }

    // Normal battery cycle.
    let mut state = hw::NodeState::load_or_init();
    let mut radio = hw::Radio::start(peripherals.WIFI, rf);
    hw::measure_and_send(&cfg, &mut sensors, &mut radio, &mut state).await;
    log_line!(
        "Awake {} ms, sleeping {} s",
        awake_since.elapsed().as_millis(),
        cfg.interval_secs
    );
    hw::deep_sleep(peripherals.LPWR, cfg.interval_secs as u64);
}
