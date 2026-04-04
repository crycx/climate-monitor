#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

#[cfg(feature = "wifi")]
extern crate alloc;

use embedded_dht_rs::dht22::Dht22;
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    delay::Delay,
    gpio::{DriveMode, Flex, OutputConfig, Pull},
};

// ── WiFi-only imports ────────────────────────────────────────────────────────
#[cfg(feature = "wifi")]
use embassy_executor::Spawner;
#[cfg(feature = "wifi")]
use embassy_net::{Config as NetConfig, Stack, StackResources, dns::DnsSocket, tcp::client::{TcpClient, TcpClientState}};
#[cfg(feature = "wifi")]
use embassy_time::{Duration, Timer};
#[cfg(feature = "wifi")]
use esp_hal::interrupt::software::SoftwareInterruptControl;
#[cfg(feature = "wifi")]
use esp_hal::timer::timg::TimerGroup;
#[cfg(feature = "wifi")]
use esp_radio::wifi::{ClientConfig, ModeConfig};
#[cfg(feature = "wifi")]
use reqwless::{
    client::{HttpClient, TlsConfig, TlsVerify},
    request::{Method, RequestBuilder},
    headers::ContentType,
};
#[cfg(feature = "wifi")]
use static_cell::StaticCell;

// This creates a default app-descriptor required by the ESP-IDF bootloader.
esp_bootloader_esp_idf::esp_app_desc!();

// ── WiFi credentials + API config (set in .cargo/config.toml [env]) ─────────
#[cfg(feature = "wifi")]
const WIFI_SSID: &str = env!("WIFI_SSID");
#[cfg(feature = "wifi")]
const WIFI_PASSWORD: &str = env!("WIFI_PASSWORD");
// Full HTTPS URL of the readings endpoint, e.g. "https://climate.sinpal.ee/api/readings"
#[cfg(feature = "wifi")]
const READINGS_URL: &str = concat!(env!("API_URL"), "readings");
#[cfg(feature = "wifi")]
const API_KEY: &str = env!("API_KEY");

// ── Static storage for embassy-net (must be 'static) ────────────────────────
#[cfg(feature = "wifi")]
static ESP_CTRL: StaticCell<esp_radio::Controller<'static>> = StaticCell::new();
#[cfg(feature = "wifi")]
static NET_RESOURCES: StaticCell<StackResources<3>> = StaticCell::new();
#[cfg(feature = "wifi")]
static NET_STACK: StaticCell<Stack<'static>> = StaticCell::new();
#[cfg(feature = "wifi")]
static TCP_STATE: StaticCell<TcpClientState<1, 4096, 4096>> = StaticCell::new();

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

    let mut dht22_pin = Flex::new(peripherals.GPIO10);
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
        delay.delay_millis(2000);
        match dht22.read() {
            Ok(r) => {
                esp_println::println!(
                    "DHT22 - Temperature: {} °C, Humidity: {} %",
                    r.temperature, r.humidity
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

/// Drives the embassy-net stack.
#[cfg(feature = "wifi")]
#[embassy_executor::task]
async fn net_task(mut runner: embassy_net::Runner<'static, esp_radio::wifi::WifiDevice<'static>>) {
    runner.run().await;
}

/// Manages WiFi connection and handles reconnects automatically.
#[cfg(feature = "wifi")]
#[embassy_executor::task]
async fn connection_task(mut wifi_ctrl: esp_radio::wifi::WifiController<'static>) {
    loop {
        esp_println::println!("WiFi: connecting to '{}'...", WIFI_SSID);
        match wifi_ctrl.connect_async().await {
            Ok(()) => {
                esp_println::println!("WiFi: connected!");
                // Wait until we lose the connection before retrying.
                wifi_ctrl.wait_for_event(esp_radio::wifi::WifiEvent::StaDisconnected).await;
                esp_println::println!("WiFi: disconnected, retrying in 5 s");
            }
            Err(e) => {
                esp_println::println!("WiFi: connect error: {:?}, retrying in 5 s", e);
            }
        }
        Timer::after(Duration::from_secs(5)).await;
    }
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

    // Initialise the radio controller (checks scheduler is running).
    let ctrl: &'static esp_radio::Controller<'static> =
        ESP_CTRL.init(esp_radio::init().unwrap());

    // Build the WiFi client interface.
    let (mut wifi_ctrl, interfaces) =
        esp_radio::wifi::new(ctrl, peripherals.WIFI, Default::default()).unwrap();

    wifi_ctrl
        .set_config(&ModeConfig::Client(
            ClientConfig::default()
                .with_ssid(WIFI_SSID.into())
                .with_password(WIFI_PASSWORD.into()),
        ))
        .unwrap();

    wifi_ctrl.start_async().await.unwrap();

    // Build the embassy-net TCP/IP stack.
    let seed = esp_hal::rng::Rng::new().random() as u64;
    let (stack, runner) = embassy_net::new(
        interfaces.sta,
        NetConfig::dhcpv4(Default::default()),
        NET_RESOURCES.init(StackResources::new()),
        seed,
    );
    let stack: &'static Stack<'static> = NET_STACK.init(stack);
    let tcp_state: &'static TcpClientState<1, 4096, 4096> = TCP_STATE.init(TcpClientState::new());

    // Spawn background tasks — connection handler and net stack runner.
    spawner.spawn(connection_task(wifi_ctrl)).unwrap();
    spawner.spawn(net_task(runner)).unwrap();

    // Wait for DHCP address.
    esp_println::println!("Waiting for IP...");
    stack.wait_config_up().await;
    if let Some(cfg) = stack.config_v4() {
        esp_println::println!("Got IP: {:?}", cfg.address);
    }

    // DHT22 on GPIO10 (open-drain, pull-up).
    let mut dht22_pin = Flex::new(peripherals.GPIO10);
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
        Timer::after(Duration::from_secs(30)).await;

        match dht22.read() {
            Ok(r) => {
                esp_println::println!(
                    "DHT22: {:.1} °C  {:.1} %",
                    r.temperature,
                    r.humidity
                );
                post_readings(stack, tcp_state, r.temperature, r.humidity).await;
            }
            Err(e) => {
                esp_println::println!("DHT22 read error: {:?}", e);
            }
        }
    }
}

/// Send temperature + humidity as HTTPS JSON POST to the backend API.
#[cfg(feature = "wifi")]
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
    let content_length = alloc::format!("{}", body.len());

    let mut rx_buf = [0u8; 512];
    match http.request(Method::POST, READINGS_URL).await {
        Ok(req) => {
            let headers = [("X-API-Key", API_KEY), ("Content-Length", content_length.as_str())];
            let mut builder = req
                .content_type(ContentType::ApplicationJson)
                .headers(&headers)
                .body(body.as_bytes());
            let result = builder.send(&mut rx_buf).await;
            match result {
                Ok(r) => esp_println::println!("POST → HTTP {}", r.status.0),
                Err(e) => esp_println::println!("POST error: {:?}", e),
            }
        }
        Err(e) => esp_println::println!("HTTP connect error: {:?}", e),
    }
}
