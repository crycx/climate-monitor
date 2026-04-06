#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

#[cfg(feature = "wifi")]
extern crate alloc;

#[cfg(feature = "wifi")]
use core::sync::atomic::{AtomicBool, Ordering};

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
use embassy_net::{Config as NetConfig, Stack, StackResources, dns::DnsSocket, tcp::client::{TcpClient, TcpClientState}, StaticConfigV4, Ipv4Address, Ipv4Cidr};
#[cfg(feature = "wifi")]
use embassy_time::{Duration, Timer};
#[cfg(feature = "wifi")]
use esp_hal::interrupt::software::SoftwareInterruptControl;
#[cfg(feature = "wifi")]
use esp_hal::timer::timg::TimerGroup;
#[cfg(feature = "wifi")]
use esp_radio::wifi::event::{self, EventExt};
#[cfg(feature = "wifi")]
use esp_radio::wifi::{ClientConfig, ModeConfig};
#[cfg(feature = "wifi")]
use reqwless::{
    client::HttpClient,
    request::{Method, RequestBuilder},
    headers::ContentType,
};
#[cfg(all(feature = "wifi", not(feature = "no-tls")))]
use reqwless::client::{TlsConfig, TlsVerify};
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
// Optional static IP (set STATIC_IP env var to use; leave empty for DHCP)
#[cfg(feature = "wifi")]
const STATIC_IP: &str = match option_env!("STATIC_IP") { Some(v) => v, None => "" };

// ── Static storage for embassy-net (must be 'static) ────────────────────────
#[cfg(feature = "wifi")]
static ESP_CTRL: StaticCell<esp_radio::Controller<'static>> = StaticCell::new();
#[cfg(feature = "wifi")]
static NET_RESOURCES: StaticCell<StackResources<3>> = StaticCell::new();
#[cfg(feature = "wifi")]
static NET_STACK: StaticCell<Stack<'static>> = StaticCell::new();
#[cfg(feature = "wifi")]
static TCP_STATE: StaticCell<TcpClientState<2, 4096, 4096>> = StaticCell::new();
#[cfg(feature = "wifi")]
static JUST_RECONNECTED: AtomicBool = AtomicBool::new(false);

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
        delay.delay_millis(10000);
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
                JUST_RECONNECTED.store(true, Ordering::Relaxed);
                // Wait until we lose the connection before retrying.
                wifi_ctrl.wait_for_event(esp_radio::wifi::WifiEvent::StaDisconnected).await;
                esp_println::println!("WiFi: disconnected, retrying in 30 s");
            }
            Err(e) => {
                esp_println::println!("WiFi: connect error: {:?}, retrying in 30 s", e);
            }
        }
        Timer::after(Duration::from_secs(30)).await;
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

    install_wifi_event_diagnostics();

    wifi_ctrl.start_async().await.unwrap();

    // Build the embassy-net TCP/IP stack.
    let seed = esp_hal::rng::Rng::new().random() as u64;
    // Use static IP if STATIC_IP env var is set, otherwise fall back to DHCP.
    let net_config = if STATIC_IP.is_empty() {
        NetConfig::dhcpv4(Default::default())
    } else {
        let parts: [u8; 4] = parse_ipv4(STATIC_IP);
        NetConfig::ipv4_static(StaticConfigV4 {
            address: Ipv4Cidr::new(Ipv4Address::new(parts[0], parts[1], parts[2], parts[3]), 24),
            gateway: Some(Ipv4Address::new(parts[0], parts[1], parts[2], 1)),
            dns_servers: Default::default(),
        })
    };
    let (stack, runner) = embassy_net::new(
        interfaces.sta,
        net_config,
        NET_RESOURCES.init(StackResources::new()),
        seed,
    );
    let stack: &'static Stack<'static> = NET_STACK.init(stack);
    let tcp_state: &'static TcpClientState<2, 4096, 4096> = TCP_STATE.init(TcpClientState::new());

    // Spawn background tasks — connection handler and net stack runner.
    spawner.spawn(connection_task(wifi_ctrl)).unwrap();
    spawner.spawn(net_task(runner)).unwrap();

    // Wait until WiFi is connected and DHCP has assigned an address.
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
                if stack.is_config_up() {
                    if JUST_RECONNECTED.load(Ordering::Relaxed) {
                        JUST_RECONNECTED.store(false, Ordering::Relaxed);
                        esp_println::println!("Post-reconnect: waiting 5 s for network to stabilise...");
                        Timer::after(Duration::from_secs(5)).await;
                    }
                    post_readings_with_retry(stack, tcp_state, r.temperature, r.humidity).await;
                } else {
                    esp_println::println!("Network down, waiting for reconnect...");
                    stack.wait_config_up().await;
                    JUST_RECONNECTED.store(false, Ordering::Relaxed);
                    esp_println::println!("Reconnected, waiting 5 s before sending...");
                    Timer::after(Duration::from_secs(5)).await;
                    post_readings_with_retry(stack, tcp_state, r.temperature, r.humidity).await;
                }
            }
            Err(e) => {
                esp_println::println!("DHT22 read error: {:?}", e);
            }
        }
    }
}

#[cfg(feature = "wifi")]
fn install_wifi_event_diagnostics() {
    event::StaStart::update_handler(|_| {
        esp_println::println!("[wifi-event] StaStart");
    });
    event::StaConnected::update_handler(|ev| {
        esp_println::println!(
            "[wifi-event] StaConnected: channel={}, authmode={}, aid={}",
            ev.channel(),
            ev.authmode(),
            ev.aid()
        );
    });
    event::StaAuthmodeChange::update_handler(|ev| {
        esp_println::println!(
            "[wifi-event] StaAuthmodeChange: old={}, new={}",
            ev.old_mode(),
            ev.new_mode()
        );
    });
    event::StaBeaconTimeout::update_handler(|_| {
        esp_println::println!("[wifi-event] StaBeaconTimeout");
    });
    event::StaDisconnected::update_handler(|ev| {
        let ssid_len = (ev.ssid_len() as usize).min(ev.ssid().len());
        let ssid = &ev.ssid()[..ssid_len];
        let bssid = ev.bssid();
        if bssid.len() >= 6 {
            esp_println::println!(
                "[wifi-event] StaDisconnected: reason={}, rssi={}, ssid={:?}, bssid={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                ev.reason(),
                ev.rssi(),
                ssid,
                bssid[0],
                bssid[1],
                bssid[2],
                bssid[3],
                bssid[4],
                bssid[5]
            );
        } else {
            esp_println::println!(
                "[wifi-event] StaDisconnected: reason={}, rssi={}, ssid={:?}, bssid={:?}",
                ev.reason(),
                ev.rssi(),
                ssid,
                bssid
            );
        }
    });
}

/// Retry wrapper: tries up to 3 times with 10 s delay between attempts.
#[cfg(feature = "wifi")]
async fn post_readings_with_retry(
    stack: &'static Stack<'static>,
    tcp_state: &'static TcpClientState<2, 4096, 4096>,
    temp: f32,
    hum: f32,
) {
    for attempt in 1..=3u8 {
        if attempt > 1 {
            esp_println::println!("Retry {}/3 in 10 s...", attempt);
            Timer::after(Duration::from_secs(10)).await;
        }
        if post_readings(stack, tcp_state, temp, hum).await {
            return;
        }
    }
    esp_println::println!("All 3 POST attempts failed, will retry next cycle.");
}

/// Send temperature + humidity as HTTPS JSON POST to the backend API.
/// Returns true on success.
#[cfg(feature = "wifi")]
async fn post_readings(
    stack: &'static Stack<'static>,
    tcp_state: &'static TcpClientState<2, 4096, 4096>,
    temp: f32,
    hum: f32,
) -> bool {
    let tcp_client = TcpClient::new(*stack, tcp_state);
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

    let body = alloc::format!(
        r#"[{{"sensor_type":"temperature","value":{:.2},"unit":"C"}},{{"sensor_type":"humidity","value":{:.2},"unit":"%"}}]"#,
        temp, hum
    );

    let mut rx_buf = [0u8; 512];
    match http.request(Method::POST, READINGS_URL).await {
        Ok(req) => {
            let headers = [("X-API-Key", API_KEY)];
            let mut builder = req
                .content_type(ContentType::ApplicationJson)
                .headers(&headers)
                .body(body.as_bytes());
            let result = builder.send(&mut rx_buf).await;
            match result {
                Ok(r) => {
                    esp_println::println!("POST → HTTP {}", r.status.0);
                    true
                }
                Err(e) => {
                    esp_println::println!("POST error: {:?}", e);
                    false
                }
            }
        }
        Err(e) => {
            esp_println::println!("HTTP connect error: {:?}", e);
            false
        }
    }
}

/// Parse a "a.b.c.d" IPv4 string into [u8; 4] at compile-time-friendly runtime.
#[cfg(feature = "wifi")]
fn parse_ipv4(s: &str) -> [u8; 4] {
    let mut out = [0u8; 4];
    let mut idx = 0;
    let mut cur = 0u8;
    let mut has_digit = false;
    for b in s.bytes() {
        if b == b'.' {
            out[idx] = cur;
            idx += 1;
            cur = 0;
            has_digit = false;
        } else {
            cur = cur * 10 + (b - b'0');
            has_digit = true;
        }
    }
    if has_digit { out[idx] = cur; }
    out
}
