#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use embedded_dht_rs::dht22::Dht22;
use embassy_executor::Spawner;
use embassy_net::{Config as NetConfig, Stack, StackResources};
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    delay::Delay,
    gpio::{DriveMode, Flex, OutputConfig, Pull},
    timer::timg::TimerGroup,
};
use esp_radio::wifi::{ClientConfig, ModeConfig, ScanConfig};
use esp_rtos as _;
use static_cell::StaticCell;

// This creates a default app-descriptor required by the ESP-IDF bootloader.
esp_bootloader_esp_idf::esp_app_desc!();

// #[panic_handler]
// fn panic(_: &core::panic::PanicInfo) -> ! {
//     loop {}
// }

const WIFI_SSID: &str = env!("WIFI_SSID");
const WIFI_PASSWORD: &str = env!("WIFI_PASSWORD");

static ESP_CTRL: StaticCell<esp_radio::Controller<'static>> = StaticCell::new();
static NET_RESOURCES: StaticCell<StackResources<3>> = StaticCell::new();
static NET_STACK: StaticCell<Stack<'static>> = StaticCell::new();

#[embassy_executor::task]
async fn net_task(mut runner: embassy_net::Runner<'static, esp_radio::wifi::WifiDevice<'static>>) {
    runner.run().await;
}

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(#[unsafe(link_section = ".dram2_uninit")] size: 64 * 1024);
    esp_alloc::heap_allocator!(size: 36 * 1024);
    esp_println::logger::init_logger_from_env();
    let delay = Delay::new();

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);
    let esp_radio_ctrl: &'static esp_radio::Controller<'static> =
        ESP_CTRL.init(esp_radio::init().unwrap());

    let (mut controller, interfaces) =
        esp_radio::wifi::new(esp_radio_ctrl, peripherals.WIFI, Default::default()).unwrap();

    controller
        .set_config(&ModeConfig::Client(
            ClientConfig::default()
                .with_ssid(WIFI_SSID.into())
                .with_password(WIFI_PASSWORD.into()),
        ))
        .unwrap();

    controller.start().unwrap();

    delay.delay_millis(1000);

    loop {
        esp_println::println!("Scanning for '{}'...", WIFI_SSID);
        match controller.scan_with_config(ScanConfig::default()) {
            Ok(results) => {
                esp_println::println!("Found {} networks:", results.len());
                for ap in &results {
                    esp_println::println!(
                        "  SSID: {}, Signal: {}, Auth: {:?}",
                        ap.ssid,
                        ap.signal_strength,
                        ap.auth_method
                    );
                }

                if results.iter().any(|ap| ap.ssid == WIFI_SSID) {
                    esp_println::println!("Network '{}' found, attempting to connect...", WIFI_SSID);
                    break;
                }
            }
            Err(error) => {
                esp_println::println!("Scan failed: {:?}", error);
            }
        }

        esp_println::println!("'{}' not found, rescanning in 3s...", WIFI_SSID);
        delay.delay_millis(3000);
    }

    esp_println::println!("Connecting to '{}'...", WIFI_SSID);

    let mut retries = 0;
    loop {
        match controller.connect() {
            Ok(()) => {}
            Err(error) => {
                esp_println::println!("connect() error: {:?}", error);
            }
        }

        delay.delay_millis(2000);

        match controller.is_connected() {
            Ok(true) => {
                esp_println::println!("Wi-Fi connected!");
                break;
            }
            Ok(false) => {
                esp_println::println!("Waiting for Wi-Fi... (attempt {})", retries);
            }
            Err(error) => {
                esp_println::println!(
                    "Wi-Fi status: {:?}, retrying... (attempt {})",
                    error,
                    retries
                );
            }
        }

        retries += 1;
        if retries > 20 {
            esp_println::println!("Failed to connect after 20 attempts, restarting...");
            esp_hal::system::software_reset();
        }
    }

    let seed = esp_hal::rng::Rng::new().random() as u64;
    let (stack, runner) = embassy_net::new(
        interfaces.sta,
        NetConfig::dhcpv4(Default::default()),
        NET_RESOURCES.init(StackResources::new()),
        seed,
    );
    let stack: &'static Stack<'static> = NET_STACK.init(stack);

    spawner.spawn(net_task(runner)).unwrap();

    esp_println::println!("Waiting for DHCP...");
    stack.wait_config_up().await;
    if let Some(cfg) = stack.config_v4() {
        esp_println::println!("Got IP: {:?}", cfg.address);
    }

    let mut dht22_pin = Flex::new(peripherals.GPIO4);
    dht22_pin.apply_output_config(
        &OutputConfig::default()
            .with_drive_mode(DriveMode::OpenDrain)
            .with_pull(Pull::None),
    );
    dht22_pin.set_output_enable(true);
    dht22_pin.set_input_enable(true);
    dht22_pin.set_high();

    let mut dht22 = Dht22::new(dht22_pin, Delay::new());

    loop {
        Timer::after(Duration::from_secs(10)).await;

        match dht22.read() {
            Ok(sensor_reading) => {
                esp_println::println!(
                    "DHT 22 Sensor - Temperature: {} °C, humidity: {} %",
                    sensor_reading.temperature,
                    sensor_reading.humidity
                );
            }
            Err(error) => {
                esp_println::dbg!("An error occurred while trying to read sensor: {:?}", error);
            }
        }
    }
}