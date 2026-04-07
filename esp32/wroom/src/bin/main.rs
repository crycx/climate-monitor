#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

extern crate alloc;

use embedded_dht_rs::dht22::Dht22;
use embassy_executor::Spawner;
use embassy_net::{
    tcp::client::TcpClientState,
    Config as NetConfig,
    Stack,
    StackResources,
};
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    delay::Delay,
    gpio::{DriveMode, Flex, OutputConfig, Pull},
    timer::timg::TimerGroup,
};
use esp_rtos as _;
use log_utils::set_time_sync_from_unix;
use network::{net_task, verify_dns};
use post_readings::post_readings;
use wifi::connect_to_wifi;

use static_cell::StaticCell;

mod log_utils;
mod network;
mod post_readings;
mod server_time_response;
mod time_utils;
mod wifi;

// This creates a default app-descriptor required by the ESP-IDF bootloader.
esp_bootloader_esp_idf::esp_app_desc!();

// #[panic_handler]
// fn panic(_: &core::panic::PanicInfo) -> ! {
//     loop {}
// }

const WIFI_SSID: &str = env!("WIFI_SSID");
const WIFI_PASSWORD: &str = env!("WIFI_PASSWORD");
const READINGS_URL: &str = env!("API_URL");
const API_KEY: &str = env!("API_KEY");
const MAX_CONSECUTIVE_POST_ERRORS: u8 = 3;
const HEARTBEAT_EVERY_CYCLES: u32 = 10;
const HTTP_CONNECT_TIMEOUT_SECS: u64 = 10;
const HTTP_SEND_TIMEOUT_SECS: u64 = 10;
const POST_TOTAL_TIMEOUT_SECS: u64 = 25;

static ESP_CTRL: StaticCell<esp_radio::Controller<'static>> = StaticCell::new();
static NET_RESOURCES: StaticCell<StackResources<3>> = StaticCell::new();
static NET_STACK: StaticCell<Stack<'static>> = StaticCell::new();
static TCP_STATE: StaticCell<TcpClientState<1, 4096, 4096>> = StaticCell::new();

// logln! macro is available at crate root due to #[macro_export]


#[allow(clippy::large_stack_frames, reason = "it's not unusual to allocate larger buffers etc. in main")]
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
    logln!("this is URL: {}", READINGS_URL);

    let esp_radio_ctrl: &'static esp_radio::Controller<'static> =
        ESP_CTRL.init(esp_radio::init().unwrap());

    let (mut controller, interfaces) =
        esp_radio::wifi::new(esp_radio_ctrl, peripherals.WIFI, Default::default()).unwrap();

    connect_to_wifi(&mut controller, &delay, WIFI_SSID, WIFI_PASSWORD);

    let seed = esp_hal::rng::Rng::new().random() as u64;
    let (stack, runner) = embassy_net::new(
        interfaces.sta,
        NetConfig::dhcpv4(Default::default()),
        NET_RESOURCES.init(StackResources::new()),
        seed,
    );
    let stack: &'static Stack<'static> = NET_STACK.init(stack);
    let tcp_state: &'static TcpClientState<1, 4096, 4096> = TCP_STATE.init(TcpClientState::new());

    spawner.spawn(net_task(runner)).unwrap();

    logln!("Waiting for DHCP...");
    match embassy_time::with_timeout(Duration::from_secs(30), stack.wait_config_up()).await {
        Ok(()) => {
            if let Some(cfg) = stack.config_v4() {
                logln!("Got IP: {:?}", cfg.address);
                logln!("Gateway: {:?}", cfg.gateway);
                logln!("DNS servers (DHCP): {:?}", cfg.dns_servers);
            }
        }
        Err(_) => {
            logln!("DHCP timeout, restarting...");
            esp_hal::system::software_reset();
        }
    }

    if !verify_dns(stack).await {
        logln!("DNS verification failed, restarting...");
        esp_hal::system::software_reset();
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
    let mut consecutive_post_errors: u8 = 0;
    let mut last_post_error: Option<alloc::string::String> = None;
    let mut loop_counter: u32 = 0;

    loop {
        Timer::after(Duration::from_secs(30)).await;
        loop_counter = loop_counter.wrapping_add(1);

        if loop_counter % HEARTBEAT_EVERY_CYCLES == 0 {
            logln!(
                "Heartbeat: wifi={:?}, has_ipv4={}",
                controller.is_connected(),
                stack.config_v4().is_some()
            );
        }

        match dht22.read() {
            Ok(sensor_reading) => {
                logln!(
                    "DHT22 Sensor - Temperature: {} °C, humidity: {} %",
                    sensor_reading.temperature,
                    sensor_reading.humidity
                );
                let post_error_str: Option<alloc::string::String> = None;
                let (post_ok, server_unix_sec, post_error) = match embassy_time::with_timeout(
                    Duration::from_secs(POST_TOTAL_TIMEOUT_SECS),
                    {
                        let body = alloc::format!(
                            r#"[{{"sensor_type":"temperature","value":{:.2},"unit":"C"}},{{"sensor_type":"humidity","value":{:.2},"unit":"%"}}]"#,
                            sensor_reading.temperature,
                            sensor_reading.humidity
                        );
                        post_readings(stack, tcp_state, body.as_bytes())
                    },
                )
                .await
                {
                    Ok(result) => result,
                    Err(_) => {
                        let msg = alloc::format!("POST total timeout after {}s", POST_TOTAL_TIMEOUT_SECS);
                        logln!("{}", msg);
                        (false, None, Some(msg))
                    }
                };

                if post_ok {
                    consecutive_post_errors = 0;
                    last_post_error = None;
                    if let Some(unix_sec) = server_unix_sec {
                        set_time_sync_from_unix(unix_sec);
                    }
                } else {
                    consecutive_post_errors = consecutive_post_errors.saturating_add(1);
                    if let Some(err) = post_error.or(post_error_str) {
                        last_post_error = Some(err);
                    }
                    logln!(
                        "POST failure count: {}/{}; last error: {}",
                        consecutive_post_errors,
                        MAX_CONSECUTIVE_POST_ERRORS,
                        last_post_error.as_deref().unwrap_or("unknown")
                    );
                    if consecutive_post_errors >= MAX_CONSECUTIVE_POST_ERRORS {
                        logln!("Too many POST failures, restarting...");
                        Delay::new().delay_millis(100);
                        esp_hal::system::software_reset();
                    }
                }
            }
            Err(error) => {
                logln!("An error occurred while trying to read sensor: {:?}", error);
            }
        }
    }
}
