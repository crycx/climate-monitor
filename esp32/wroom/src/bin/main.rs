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
    i2c::master::{Config as I2cConfig, I2c},
    time::Rate,
    timer::timg::TimerGroup,
};
use esp_rtos as _;
use log_utils::set_time_sync_from_unix;
use network::{net_task, verify_dns};
use post_readings::post_readings;
use ssd1306::{I2CDisplayInterface, Ssd1306, prelude::*};
use wifi::connect_to_wifi;

use static_cell::StaticCell;

mod log_utils;
mod body;
mod dht22;
mod network;
mod post_readings;
mod server_time_response;
mod time_utils;
mod wifi;

#[path = "../oled_display.rs"]
mod oled_display;

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

fn oled_is_portrait() -> bool {
    matches!(option_env!("OLED_ORIENTATION"), Some("portrait") | Some("PORTRAIT"))
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
            logln!("OLED: set landscape rotation failed: {:?}", e);
        }

        if let Err(e) = oled_display::draw_status(display, status) {
            logln!("OLED: draw_status failed: {:?}", e);
        }

        if let Err(e) = display.set_rotation(DisplayRotation::Rotate90) {
            logln!("OLED: restore portrait rotation failed: {:?}", e);
        }
    } else if let Err(e) = oled_display::draw_status(display, status) {
        logln!("OLED: draw_status failed: {:?}", e);
    }
}

fn oled_i2c_address() -> u8 {
    match option_env!("OLED_I2C_ADDR") {
        Some("0x3D") | Some("0X3D") | Some("61") => 0x3D,
        _ => 0x3C,
    }
}

static ESP_CTRL: StaticCell<esp_radio::Controller<'static>> = StaticCell::new();
static NET_RESOURCES: StaticCell<StackResources<3>> = StaticCell::new();
static NET_STACK: StaticCell<Stack<'static>> = StaticCell::new();
static TCP_STATE: StaticCell<TcpClientState<1, 4096, 4096>> = StaticCell::new();

// logln! macro is available at crate root due to #[macro_export]


#[allow(clippy::large_stack_frames, reason = "it's not unusual to allocate larger buffers etc. in main")]
#[esp_rtos::main]
async fn main(spawner: Spawner) {
    let oled_portrait_mode = oled_is_portrait();
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(#[unsafe(link_section = ".dram2_uninit")] size: 64 * 1024);
    esp_alloc::heap_allocator!(size: 36 * 1024);
    esp_println::logger::init_logger_from_env();
    let delay = Delay::new();
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);
    logln!("this is URL: {}", READINGS_URL);

    let oled_addr = oled_i2c_address();
    let mut oled = match I2c::new(
        peripherals.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(100)),
    ) {
        Ok(i2c) => {
            let mut i2c = i2c.with_sda(peripherals.GPIO21).with_scl(peripherals.GPIO22);
            let mut found_any = false;
            logln!("I2C scan start (GPIO21 SDA, GPIO22 SCL)...");
            for addr in 0x08u8..=0x77u8 {
                if i2c.write(addr, &[]).is_ok() {
                    found_any = true;
                    logln!("I2C: found device at 0x{:02X}", addr);
                }
            }
            if !found_any {
                logln!("I2C scan: no device ACK on 0x08..0x77");
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
            logln!("OLED: I2C bus init failed: {:?}", e);
            None
        }
    };

    if let Some(display) = oled.as_mut() {
        if oled_portrait_mode {
            if let Err(e) = display.set_rotation(DisplayRotation::Rotate90) {
                logln!("OLED: initial portrait rotation failed: {:?}", e);
            }
        }

        match oled_display::init_screen(display, "Booting WiFi...") {
            Ok(()) => {
                if oled_portrait_mode {
                    draw_status_for_mode(display, "Booting WiFi...", oled_portrait_mode);
                }
                logln!("OLED: init OK at I2C address 0x{:02X}", oled_addr);
            }
            Err(e) => {
                logln!("OLED: init failed at I2C address 0x{:02X}: {:?}", oled_addr, e);
                oled = None;
            }
        }
    } else {
        logln!("OLED: disabled (bus init failed)");
    }

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

    if !verify_dns(stack, controller.is_connected().ok()).await {
        logln!("DNS verification failed, restarting...");
        esp_hal::system::software_reset();
    }

    let dht22_pin = dht22::init_dht22_pin(peripherals.GPIO4);
    let mut dht22 = Dht22::new(dht22_pin, Delay::new());
    let mut consecutive_post_errors: u8 = 0;
    let mut last_post_error: Option<alloc::string::String> = None;
    let mut last_sent_secs: usize = 0;
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

        match dht22::read(&mut dht22) {
            Ok(sensor_reading) => {
                logln!(
                    "DHT22 Sensor - Temperature: {} °C, humidity: {} %",
                    sensor_reading.temperature,
                    sensor_reading.humidity
                );
                if let Some(display) = oled.as_mut() {
                    let total_secs = (embassy_time::Instant::now().as_millis() / 1000) as usize;
                    let hours = ((total_secs / 3600) % 24) as u8;
                    let minutes = ((total_secs % 3600) / 60) as u8;
                    let data_sent = total_secs.saturating_sub(last_sent_secs) < 5;
                    let _ = oled_display::show_reading(
                        display,
                        sensor_reading.temperature,
                        sensor_reading.humidity,
                        hours,
                        minutes,
                        data_sent,
                    );
                }
                let post_error_str: Option<alloc::string::String> = None;
                let values = [
                    body::SensorValue {
                        sensor_type: "temperature",
                        value: sensor_reading.temperature,
                        unit: "C",
                    },
                    body::SensorValue {
                        sensor_type: "humidity",
                        value: sensor_reading.humidity,
                        unit: "%",
                    },
                ];
                let body = body::build_body(&values);
                let wifi_connected = controller.is_connected().ok();
                let (post_ok, server_unix_sec, post_error) = match embassy_time::with_timeout(
                    Duration::from_secs(POST_TOTAL_TIMEOUT_SECS),
                    post_readings(stack, tcp_state, body.as_bytes(), wifi_connected),
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
                    let secs = (embassy_time::Instant::now().as_millis() / 1000) as usize;
                    last_sent_secs = secs;
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
