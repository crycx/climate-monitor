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
    dns::DnsQueryType,
    dns::DnsSocket,
    tcp::client::{TcpClient, TcpClientState},
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
use esp_radio::wifi::{ClientConfig, ModeConfig, ScanConfig};
use esp_rtos as _;
use reqwless::{
    client::{HttpClient, TlsConfig, TlsVerify},
    headers::ContentType,
    request::{Method, RequestBuilder},
};
use log_utils::set_time_sync_from_unix;
use server_time_response::extract_server_unix_from_headers;

use static_cell::StaticCell;

mod log_utils;
mod server_time_response;
mod time_utils;

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

#[embassy_executor::task]
async fn net_task(mut runner: embassy_net::Runner<'static, esp_radio::wifi::WifiDevice<'static>>) {
    runner.run().await;
}

async fn verify_dns(stack: &'static Stack<'static>) -> bool {
    let hostname = {
        let after_scheme = READINGS_URL
            .find("://")
            .map(|i| &READINGS_URL[i + 3..])
            .unwrap_or(READINGS_URL);
        after_scheme
            .find('/')
            .map(|i| &after_scheme[..i])
            .unwrap_or(after_scheme)
    };

    logln!("Resolving {} ...", hostname);
    match stack.dns_query(hostname, DnsQueryType::A).await {
        Ok(addresses) => {
            logln!("DNS resolved: {:?}", addresses);
            true
        }
        Err(error) => {
            logln!("DNS query failed: {:?}", error);
            false
        }
    }
}

async fn post_readings(
    stack: &'static Stack<'static>,
    tcp_state: &'static TcpClientState<1, 4096, 4096>,
    temp: f32,
    hum: f32,
) -> (bool, Option<u64>, Option<alloc::string::String>) {
    let tcp_client = TcpClient::new(*stack, tcp_state);
    let dns = DnsSocket::new(*stack);

    let tls_seed = esp_hal::rng::Rng::new().random() as u64;
    let mut tls_rx = [0u8; 4096];
    let mut tls_tx = [0u8; 4096];
    let tls = TlsConfig::new(tls_seed, &mut tls_rx, &mut tls_tx, TlsVerify::None);

    let mut http = HttpClient::new_with_tls(&tcp_client, &dns, tls);

    let body = alloc::format!(
        r#"[{{"sensor_type":"temperature","value":{:.2},"unit":"C"}},{{"sensor_type":"humidity","value":{:.2},"unit":"%"}}]"#,
        temp, hum
    );
    let mut rx_buf = [0u8; 512];

    match embassy_time::with_timeout(
        Duration::from_secs(HTTP_CONNECT_TIMEOUT_SECS),
        http.request(Method::POST, READINGS_URL),
    )
    .await
    {
        Err(_) => {
            let msg = alloc::format!("HTTP connect timeout after {}s", HTTP_CONNECT_TIMEOUT_SECS);
            logln!("{}", msg);
            (false, None, Some(msg))
        }
        Ok(request) => {
            let request = match request {
                Ok(request) => request,
                Err(error) => {
                    let msg = alloc::format!("HTTP connect error: {:?}", error);
                    logln!("{}", msg);
                    return (false, None, Some(msg));
                }
            };

            let headers = [("X-API-Key", API_KEY)];
            let mut builder = request
                .content_type(ContentType::ApplicationJson)
                .headers(&headers)
                .body(body.as_bytes());
            match embassy_time::with_timeout(
                Duration::from_secs(HTTP_SEND_TIMEOUT_SECS),
                builder.send(&mut rx_buf),
            )
            .await
            {
                Err(_) => {
                    let msg = alloc::format!("POST send timeout after {}s", HTTP_SEND_TIMEOUT_SECS);
                    logln!("{}", msg);
                    (false, None, Some(msg))
                }
                Ok(response) => {
                    match response {
                        Ok(response) => {
                            let sec = extract_server_unix_from_headers(response.headers());
                            logln!("POST -> HTTP {}", response.status.0);
                            return (true, sec, None);
                        }
                        Err(error) => {
                            let msg = alloc::format!("POST error: {:?}", error);
                            logln!("{}", msg);
                            (false, None, Some(msg))
                        }
                    }
                }
            }
        }
        // Err(error) => {
        //     esp_println::println!("HTTP connect error: {:?}", error);
        // }
    }
}

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
        logln!("Scanning for '{}'...", WIFI_SSID);
        match controller.scan_with_config(ScanConfig::default()) {
            Ok(results) => {
                logln!("Found {} networks:", results.len());
                for ap in &results {
                    logln!(
                        "  SSID: {}, Signal: {}, Auth: {:?}",
                        ap.ssid,
                        ap.signal_strength,
                        ap.auth_method
                    );
                }

                if results.iter().any(|ap| ap.ssid == WIFI_SSID) {
                    logln!("Network '{}' found, attempting to connect...", WIFI_SSID);
                    break;
                }
            }
            Err(error) => {
                logln!("Scan failed: {:?}", error);
            }
        }

        logln!("'{}' not found, rescanning in 3s...", WIFI_SSID);
        delay.delay_millis(3000);
    }

    logln!("Connecting to '{}'...", WIFI_SSID);

    let mut retries = 0;
    loop {
        match controller.connect() {
            Ok(()) => {}
            Err(error) => {
                logln!("connect() error: {:?}", error);
            }
        }

        delay.delay_millis(2000);

        match controller.is_connected() {
            Ok(true) => {
                logln!("Wi-Fi connected!");
                break;
            }
            Ok(false) => {
                logln!("Waiting for Wi-Fi... (attempt {})", retries);
            }
            Err(error) => {
                logln!(
                    "Wi-Fi status: {:?}, retrying... (attempt {})",
                    error,
                    retries
                );
            }
        }

        retries += 1;
        if retries > 20 {
            logln!("Failed to connect after 20 attempts, restarting...");
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
                let mut post_error_str: Option<alloc::string::String> = None;
                let (post_ok, server_unix_sec, post_error) = match embassy_time::with_timeout(
                    Duration::from_secs(POST_TOTAL_TIMEOUT_SECS),
                    post_readings(
                        stack,
                        tcp_state,
                        sensor_reading.temperature,
                        sensor_reading.humidity,
                    ),
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