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
    ConfigV4,
    Ipv4Address,
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
use static_cell::StaticCell;

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

static ESP_CTRL: StaticCell<esp_radio::Controller<'static>> = StaticCell::new();
static NET_RESOURCES: StaticCell<StackResources<3>> = StaticCell::new();
static NET_STACK: StaticCell<Stack<'static>> = StaticCell::new();
static TCP_STATE: StaticCell<TcpClientState<1, 4096, 4096>> = StaticCell::new();

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

    esp_println::println!("Resolving {} ...", hostname);
    match stack.dns_query(hostname, DnsQueryType::A).await {
        Ok(addresses) => {
            esp_println::println!("DNS resolved: {:?}", addresses);
            true
        }
        Err(error) => {
            esp_println::println!("DNS query failed: {:?}", error);
            false
        }
    }
}

async fn post_readings(
    stack: &'static Stack<'static>,
    tcp_state: &'static TcpClientState<1, 4096, 4096>,
    temp: f32,
    hum: f32,
) {
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

    match http.request(Method::POST, READINGS_URL).await {
        Ok(request) => {
            let headers = [("X-API-Key", API_KEY)];
            let mut builder = request
                .content_type(ContentType::ApplicationJson)
                .headers(&headers)
                .body(body.as_bytes());
            match builder.send(&mut rx_buf).await {
                Ok(response) => {
                    esp_println::println!("POST -> HTTP {}", response.status.0);
                }
                Err(error) => {
                    esp_println::println!("POST error: {:?}", error);
                }
            }
        }
        Err(error) => {
            esp_println::println!("HTTP connect error: {:?}", error);
        }
    }
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
    esp_println::println!("this is URL: {}", READINGS_URL);


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
    let tcp_state: &'static TcpClientState<1, 4096, 4096> = TCP_STATE.init(TcpClientState::new());

    spawner.spawn(net_task(runner)).unwrap();

    esp_println::println!("Waiting for DHCP...");
    match embassy_time::with_timeout(Duration::from_secs(30), stack.wait_config_up()).await {
        Ok(()) => {
            if let Some(cfg) = stack.config_v4() {
                esp_println::println!("Got IP: {:?}", cfg.address);
                esp_println::println!("Gateway: {:?}", cfg.gateway);
                esp_println::println!("DNS servers (DHCP): {:?}", cfg.dns_servers);

                // Override DNS to public resolvers
                let mut override_cfg = cfg.clone();
                override_cfg.dns_servers.clear();
                let _ = override_cfg.dns_servers.push(Ipv4Address::new(8, 8, 8, 8));
                let _ = override_cfg.dns_servers.push(Ipv4Address::new(1, 1, 1, 1));
                stack.set_config_v4(ConfigV4::Static(override_cfg));
                esp_println::println!("DNS overridden to [8.8.8.8, 1.1.1.1]");
                esp_println::println!("Got IP: {:?}", cfg.address);
            }
        }
        Err(_) => {
            esp_println::println!("DHCP timeout, restarting...");
            esp_hal::system::software_reset();
        }
    }

    if !verify_dns(stack).await {
        esp_println::println!("DNS verification failed, restarting...");
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

    loop {
        Timer::after(Duration::from_secs(30)).await;

        match dht22.read() {
            Ok(sensor_reading) => {
                esp_println::println!(
                    "DHT 22 Sensor - Temperature: {} °C, humidity: {} %",
                    sensor_reading.temperature,
                    sensor_reading.humidity
                );
                post_readings(stack, tcp_state, sensor_reading.temperature, sensor_reading.humidity).await;
            }
            Err(error) => {
                esp_println::dbg!("An error occurred while trying to read sensor: {:?}", error);
            }
        }
    }
}