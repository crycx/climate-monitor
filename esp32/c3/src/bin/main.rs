#![no_std]
#![no_main]
#![cfg_attr(feature = "wifi", feature(impl_trait_in_assoc_type))]
#![allow(unexpected_cfgs)]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

/// Panic handler: prindib vea consolesse ja vilgub GPIO8 LED-i kiiresti (100 ms).
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    esp_println::println!("\n\n*** PANIC ***\n{}", info);

    // Steal peripherals — normaalne omandus puudub paanika ajal.
    let p = unsafe { esp_hal::peripherals::Peripherals::steal() };
    let mut led = esp_hal::gpio::Output::new(p.GPIO8, esp_hal::gpio::Level::High, esp_hal::gpio::OutputConfig::default());
    let delay = esp_hal::delay::Delay::new();
    loop {
        led.toggle();
        delay.delay_millis(100);
    }
}

#[cfg(feature = "wifi")]
extern crate alloc;

#[cfg(feature = "wifi")]
use alloc::{format, string::String};
#[cfg(feature = "wifi")]
use core::sync::atomic::{AtomicBool, Ordering};

use embedded_dht_rs::dht22::Dht22;
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    delay::Delay,
    gpio::{DriveMode, Flex, OutputConfig, Pull},
};
#[cfg(feature = "wifi")]
use esp_hal::gpio::{Input, InputConfig, Level, Output};

// ── WiFi-only imports ────────────────────────────────────────────────────────
#[cfg(feature = "wifi")]
use embassy_executor::Spawner;
#[cfg(feature = "wifi")]
use embassy_net::{Stack, dns::DnsSocket, tcp::client::TcpClientState};
#[cfg(feature = "wifi")]
use embassy_time::{Duration, Instant, Timer};
#[cfg(feature = "wifi")]
use esp_hal::interrupt::software::SoftwareInterruptControl;
#[cfg(feature = "wifi")]
use esp_hal::timer::timg::TimerGroup;
#[cfg(feature = "wifi")]
use esp_storage::FlashStorage;
#[cfg(feature = "wifi")]
use esp_wifi_caddy::WifiCaddyCommand;
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
#[cfg(feature = "wifi")]
use wifi_caddy_proc::WifiCaddyConfig;

// This creates a default app-descriptor required by the ESP-IDF bootloader.
esp_bootloader_esp_idf::esp_app_desc!();

// ── Flash-backed runtime config ──────────────────────────────────────────────
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Default, WifiCaddyConfig)]
#[config_server]
#[config_notify]
#[config_ui(
    page_heading = "Climate Monitor Setup",
        subtitle = "Configure WiFi and API access for this device",
        extra_js = r#"
(function () {
    function installWifiScanUi() {
        var ssidInput = document.getElementById('wifi_ssid');
        if (!ssidInput || document.getElementById('wifi_scan_select')) {
            return;
        }

        var field = ssidInput.closest('.config-form-group');
        if (!field) {
            return;
        }

        var row = document.createElement('div');
        row.style.display = 'flex';
        row.style.gap = '8px';
        row.style.marginTop = '8px';
        row.style.alignItems = 'center';

        var scanBtn = document.createElement('button');
        scanBtn.type = 'button';
        scanBtn.textContent = 'Scan nearby WiFi';

        var select = document.createElement('select');
        select.id = 'wifi_scan_select';
        select.style.flex = '1';

        var placeholder = document.createElement('option');
        placeholder.value = '';
        placeholder.textContent = 'Select a network';
        select.appendChild(placeholder);

        var status = document.createElement('div');
        status.className = 'config-form-help';
        status.style.marginTop = '6px';

        function populate(networks) {
            select.innerHTML = '';

            var initial = document.createElement('option');
            initial.value = '';
            initial.textContent = networks.length ? 'Select a network' : 'No networks found yet';
            select.appendChild(initial);

            networks.forEach(function (network) {
                var option = document.createElement('option');
                option.value = network.ssid;
                option.textContent = network.ssid + ' (' + network.rssi + ' dBm)';
                select.appendChild(option);
            });
        }

        function scanNetworks() {
            scanBtn.disabled = true;
            status.textContent = 'Scanning nearby networks...';

            fetch('/wifi-scan')
                .then(function (response) {
                    if (!response.ok) {
                        throw new Error('HTTP ' + response.status);
                    }
                    return response.json();
                })
                .then(function (data) {
                    var networks = Array.isArray(data.networks) ? data.networks.filter(function (item) {
                        return item && item.ssid;
                    }) : [];
                    populate(networks);
                    status.textContent = networks.length
                        ? 'Found ' + networks.length + ' nearby network(s).'
                        : 'Scan completed, but no visible networks were returned yet.';
                })
                .catch(function (error) {
                    status.textContent = 'Scan failed: ' + error.message;
                })
                .finally(function () {
                    scanBtn.disabled = false;
                });
        }

        select.addEventListener('change', function () {
            if (select.value) {
                ssidInput.value = select.value;
            }
        });

        scanBtn.addEventListener('click', scanNetworks);
        row.appendChild(scanBtn);
        row.appendChild(select);
        field.appendChild(row);
        field.appendChild(status);

        var logToggle = document.createElement('button');
        logToggle.type = 'button';
        logToggle.textContent = 'Show device log';
        logToggle.style.marginTop = '8px';

        var logPanel = document.createElement('div');
        logPanel.style.display = 'none';
        logPanel.style.marginTop = '8px';
        logPanel.style.border = '1px solid #cbd5e1';
        logPanel.style.borderRadius = '8px';
        logPanel.style.background = '#0b1220';
        logPanel.style.color = '#dbeafe';
        logPanel.style.padding = '8px';

        var logText = document.createElement('pre');
        logText.style.margin = '0';
        logText.style.maxHeight = '180px';
        logText.style.overflow = 'auto';
        logText.style.whiteSpace = 'pre-wrap';
        logText.style.fontFamily = 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace';
        logText.style.fontSize = '12px';
        logText.textContent = 'No log data yet.';
        logPanel.appendChild(logText);

        var logTimer = null;

        function loadLog() {
            fetch('/wifi-log')
                .then(function (response) {
                    if (!response.ok) {
                        throw new Error('HTTP ' + response.status);
                    }
                    return response.text();
                })
                .then(function (text) {
                    logText.textContent = text && text.trim() ? text : 'No log data yet.';
                })
                .catch(function (error) {
                    logText.textContent = 'Log fetch failed: ' + error.message;
                });
        }

        function openLogPanel() {
            logPanel.style.display = '';
            logToggle.textContent = 'Hide device log';
            fetch('/wifi-log-enable')
                .catch(function () {
                    // Best-effort; still try to show whatever log is available.
                })
                .finally(function () {
                    loadLog();
                    if (logTimer === null) {
                        logTimer = window.setInterval(loadLog, 5000);
                    }
                });
        }

        function closeLogPanel() {
            logPanel.style.display = 'none';
            logToggle.textContent = 'Show device log';
            if (logTimer !== null) {
                window.clearInterval(logTimer);
                logTimer = null;
            }
            fetch('/wifi-log-disable').catch(function () {
                // Best-effort disable.
            });
        }

        logToggle.addEventListener('click', function () {
            if (logPanel.style.display === 'none') {
                openLogPanel();
            } else {
                closeLogPanel();
            }
        });

        field.appendChild(logToggle);
        field.appendChild(logPanel);

        scanNetworks();
    }

    var originalLoadMain = window.loadConfig_main;
    if (typeof originalLoadMain === 'function') {
        window.loadConfig_main = function () {
            return Promise.resolve(originalLoadMain()).then(function (result) {
                installWifiScanUi();
                return result;
            });
        };
    }

    window.addEventListener('load', installWifiScanUi);
})();
"#
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
    #[config_form(fieldset = "Backend", input_type = "text", help = "Device API key")]
    api_key: String,

    #[config_store(env_default = "WIFI_SSID_2", notify = "Wifi")]
    #[config_form(fieldset = "WiFi slot 2", label = "SSID", help = "Optional second network name")]
    wifi_ssid_2: String,

    #[config_store(env_default = "WIFI_PASSWORD_2", notify = "Wifi")]
    #[config_form(fieldset = "WiFi slot 2", label = "Password", input_type = "password", help = "Optional second network password")]
    wifi_password_2: String,

    #[config_store(env_default = "WIFI_SSID_3", notify = "Wifi")]
    #[config_form(fieldset = "WiFi slot 3", label = "SSID", help = "Optional third network name")]
    wifi_ssid_3: String,

    #[config_store(env_default = "WIFI_PASSWORD_3", notify = "Wifi")]
    #[config_form(fieldset = "WiFi slot 3", label = "Password", input_type = "password", help = "Optional third network password")]
    wifi_password_3: String,

    #[config_store(env_default = "TEMP_OFFSET")]
    #[config_form(fieldset = "Calibration", label = "Temperature offset (°C)", help = "Added to raw reading. Positive = suuremaks, negatiivne = väiksemaks. Näiteks -1.7")]
    temp_offset: String,

    #[config_store(env_default = "HUM_OFFSET")]
    #[config_form(fieldset = "Calibration", label = "Humidity offset (%)", help = "Added to raw reading. Positive = suuremaks, negatiivne = väiksemaks. Näiteks -7.0")]
    hum_offset: String,
}

// ── Static storage for embassy-net (must be 'static) ────────────────────────
#[cfg(feature = "wifi")]
static NET_STACK: StaticCell<Stack<'static>> = StaticCell::new();
#[cfg(feature = "wifi")]
static TCP_STATE: StaticCell<embassy_net::tcp::client::TcpClientState<2, 4096, 4096>> = StaticCell::new();
#[cfg(feature = "wifi")]
static BUTTON_PRESSED: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "wifi")]
static AP_ACTIVE: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "wifi")]
static SENDING: AtomicBool = AtomicBool::new(false);

#[cfg(feature = "wifi")]
fn log_runtime_line(line: &str) {
    esp_println::println!("{}", line);
    esp_wifi_caddy::portal_log(line);
}

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

    let mut dht22_pin = Flex::new(peripherals.GPIO5);
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
                let temp = r.temperature - 1.7;
                let hum = (r.humidity - 7.0).max(0.0);
                esp_println::println!(
                    "DHT22 - Temperature: {:.1} °C, Humidity: {:.1} %",
                    temp, hum
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

#[cfg(feature = "wifi")]
#[embassy_executor::task]
async fn led_task(mut led: Output<'static>) {
    loop {
        if AP_ACTIVE.load(Ordering::Relaxed) {
            // AP portal aktiivne: vilgub aeglaselt 500 ms (aktiiv-LOW: low=sees)
            led.set_low();
            Timer::after(Duration::from_millis(500)).await;
            led.set_high();
            Timer::after(Duration::from_millis(500)).await;
        } else if SENDING.load(Ordering::Relaxed) {
            // Andmete saatmine: LED püsivalt sees
            led.set_low();
            Timer::after(Duration::from_millis(50)).await;
        } else {
            // Ooteseisund: LED väljas
            led.set_high();
            Timer::after(Duration::from_millis(100)).await;
        }
    }
}

#[cfg(feature = "wifi")]
#[embassy_executor::task]
async fn gpio_poll_task(btn: Input<'static>) {
    loop {
        Timer::after(Duration::from_millis(10)).await;
        BUTTON_PRESSED.store(btn.level() == Level::Low, Ordering::Relaxed);
    }
}

#[cfg(feature = "wifi")]
#[embassy_executor::task]
async fn button_task(wifi_sender: esp_wifi_caddy::WifiCommandSender) {
    loop {
        // Wait for press.
        while !BUTTON_PRESSED.load(Ordering::Relaxed) {
            Timer::after(Duration::from_millis(20)).await;
        }
        let press_start = Instant::now();

        // Wait for release.
        while BUTTON_PRESSED.load(Ordering::Relaxed) {
            Timer::after(Duration::from_millis(20)).await;
        }
        let held = press_start.elapsed();

        if held >= Duration::from_secs(3) {
            log_runtime_line("Button: long press -> force AP on");
            AP_ACTIVE.store(true, Ordering::Relaxed);
            wifi_sender
                .send(WifiCaddyCommand::APUp("ClimateMonitor-".into()))
                .await;
        } else if AP_ACTIVE.load(Ordering::Relaxed) {
            log_runtime_line("Button: short press -> AP portal off");
            AP_ACTIVE.store(false, Ordering::Relaxed);
            wifi_sender.send(WifiCaddyCommand::APDown).await;
        } else {
            log_runtime_line("Button: short press -> AP portal on");
            AP_ACTIVE.store(true, Ordering::Relaxed);
            wifi_sender
                .send(WifiCaddyCommand::APUp("ClimateMonitor-".into()))
                .await;
        }

        Timer::after(Duration::from_millis(300)).await;
    }
}

/// Selects the best WiFi credential to use.
///
/// If scan results are available, prefers the first slot whose SSID is
/// currently visible over-the-air (strongest known network wins in slot order).
/// Falls back to the first non-empty slot when no scan data is present.
#[cfg(feature = "wifi")]
fn find_best_ssid(cfg: &AppConfig) -> Option<(String, String)> {
    let slots = [
        (cfg.wifi_ssid.as_str(), cfg.wifi_password.as_str()),
        (cfg.wifi_ssid_2.as_str(), cfg.wifi_password_2.as_str()),
        (cfg.wifi_ssid_3.as_str(), cfg.wifi_password_3.as_str()),
    ];
    // Prefer a network that is visible in the latest scan results.
    for (ssid, pass) in slots {
        if !ssid.is_empty() && esp_wifi_caddy::scan_ssid_visible(ssid) {
            return Some((String::from(ssid), String::from(pass)));
        }
    }
    // Fall back to the first configured slot.
    slots
        .into_iter()
        .find(|(ssid, _)| !ssid.is_empty())
        .map(|(ssid, pass)| (String::from(ssid), String::from(pass)))
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

    // GPIO9 = BOOT button (active-low).
    let btn = Input::new(peripherals.GPIO9, InputConfig::default().with_pull(Pull::Up));
    spawner.spawn(gpio_poll_task(btn)).unwrap();

    // GPIO8 = built-in LED, vilgub kui AP portal on aktiivne.
    let led = Output::new(peripherals.GPIO8, Level::High, OutputConfig::default());
    spawner.spawn(led_task(led)).unwrap();

    let flash = FlashStorage::new(peripherals.FLASH);
    let (wifi_stacks, wifi_sender, config_handle, config_rx) =
        esp_wifi_caddy::wifi_init!(AppConfig, spawner, peripherals.WIFI, flash, "config")
            .expect("wifi_init failed");

    spawner.spawn(button_task(wifi_sender.clone())).unwrap();

    let stack: &'static Stack<'static> = NET_STACK.init(wifi_stacks.sta);
    let tcp_state: &'static TcpClientState<2, 4096, 4096> = TCP_STATE.init(TcpClientState::new());

    // Startup: connect directly to the first configured WiFi network.
    {
        let best = {
            let cfg = config_handle.config().lock().await;
            find_best_ssid(&cfg)
        };

        if let Some((ssid, pass)) = best {
            log_runtime_line(format!("Config found: connecting to {}", ssid).as_str());
            wifi_sender.send(WifiCaddyCommand::StaUp(ssid, pass)).await;
        } else {
            log_runtime_line("No WiFi config: starting AP portal at 192.168.2.1");
            AP_ACTIVE.store(true, Ordering::Relaxed);
            wifi_sender
                .send(WifiCaddyCommand::APUp("ClimateMonitor-".into()))
                .await;
        }
    }

    // DHT22 on GPIO5 (open-drain, pull-up).
    let mut dht22_pin = Flex::new(peripherals.GPIO5);
    dht22_pin.apply_output_config(
        &OutputConfig::default()
            .with_drive_mode(DriveMode::OpenDrain)
            .with_pull(Pull::Up),
    );
    dht22_pin.set_output_enable(true);
    dht22_pin.set_input_enable(true);
    dht22_pin.set_high();

    let mut dht22 = Dht22::new(dht22_pin, Delay::new());

    let mut net_was_up = stack.is_config_up();
    loop {
        Timer::after(Duration::from_secs(30)).await;

        let net_is_up_now = stack.is_config_up();
        if net_is_up_now && !net_was_up {
            log_runtime_line("WiFi connected");
            if let Some(cfg) = stack.config_v4() {
                log_runtime_line(format!("Got IP: {:?}", cfg.address).as_str());
            }
        } else if !net_is_up_now && net_was_up {
            log_runtime_line("WiFi disconnected");
        }
        net_was_up = net_is_up_now;

        if let Ok(_changed) = config_rx.try_receive() {
            // AP is still up (user was in the portal), so scan results are fresh.
            // Pick the best visible network from all configured slots.
            let best = {
                let cfg = config_handle.config().lock().await;
                find_best_ssid(&cfg)
            };
            if let Some((ssid, pass)) = best {
                log_runtime_line(format!("Config changed: connecting to {}", ssid).as_str());
                wifi_sender.send(WifiCaddyCommand::StaUp(ssid, pass)).await;

                if AP_ACTIVE.load(Ordering::Relaxed) {
                    AP_ACTIVE.store(false, Ordering::Relaxed);
                    wifi_sender.send(WifiCaddyCommand::APDown).await;
                }
            }
        }

        match dht22.read() {
            Ok(r) => {
                let (temp_offset, hum_offset, api_url, api_key) = {
                    let cfg = config_handle.config().lock().await;
                    let t: f32 = cfg.temp_offset.parse().unwrap_or(-1.7);
                    let h: f32 = cfg.hum_offset.parse().unwrap_or(-7.0);
                    (t, h, cfg.api_url.clone(), cfg.api_key.clone())
                };
                let temp = r.temperature + temp_offset;
                let hum = (r.humidity + hum_offset).max(0.0);
                log_runtime_line(format!("DHT22: {:.1} C  {:.1} %", temp, hum).as_str());
                if stack.is_config_up() {
                    post_readings_with_retry(
                        stack,
                        tcp_state,
                        api_url.as_str(),
                        api_key.as_str(),
                        temp,
                        hum,
                    )
                    .await;
                } else {
                    log_runtime_line("Network down, waiting for reconnect...");
                    let reconnected = embassy_time::with_timeout(
                        Duration::from_secs(60),
                        stack.wait_config_up(),
                    )
                    .await
                    .is_ok();
                    if reconnected {
                        log_runtime_line("Reconnected, waiting 5 s before sending...");
                        Timer::after(Duration::from_secs(5)).await;
                        post_readings_with_retry(
                            stack,
                            tcp_state,
                            api_url.as_str(),
                            api_key.as_str(),
                            temp,
                            hum,
                        )
                        .await;
                    } else {
                        log_runtime_line("Reconnect timed out after 60 s, will retry next cycle.");
                    }
                }
            }
            Err(e) => {
                log_runtime_line(format!("DHT22 read error: {:?}", e).as_str());
            }
        }
    }
}

/// Retry wrapper: tries up to 3 times with 10 s delay between attempts.
#[cfg(feature = "wifi")]
async fn post_readings_with_retry(
    stack: &'static Stack<'static>,
    tcp_state: &'static TcpClientState<2, 4096, 4096>,
    api_url: &str,
    api_key: &str,
    temp: f32,
    hum: f32,
) {
    SENDING.store(true, Ordering::Relaxed);
    for attempt in 1..=3u8 {
        if attempt > 1 {
            log_runtime_line(format!("Retry {}/3 in 10 s...", attempt).as_str());
            Timer::after(Duration::from_secs(10)).await;
        }
        if post_readings(stack, tcp_state, api_url, api_key, temp, hum).await {
            SENDING.store(false, Ordering::Relaxed);
            return;
        }
    }
    SENDING.store(false, Ordering::Relaxed);
    log_runtime_line("All 3 POST attempts failed, will retry next cycle.");
}

/// Send temperature + humidity as HTTPS JSON POST to the backend API.
/// Returns true on success.
#[cfg(feature = "wifi")]
async fn post_readings(
    stack: &'static Stack<'static>,
    tcp_state: &'static TcpClientState<2, 4096, 4096>,
    api_url: &str,
    api_key: &str,
    temp: f32,
    hum: f32,
) -> bool {
    if api_url.is_empty() || api_key.is_empty() {
        log_runtime_line("API config missing, skipping POST.");
        return false;
    }

    let tcp_client = embassy_net::tcp::client::TcpClient::new(*stack, tcp_state);
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

    let body = format!(
        r#"[{{"sensor_type":"temperature","value":{:.2},"unit":"C"}},{{"sensor_type":"humidity","value":{:.2},"unit":"%"}}]"#,
        temp, hum
    );
    let readings_url = format!("{}readings", api_url);

    let mut rx_buf = [0u8; 512];
    match http.request(Method::POST, readings_url.as_str()).await {
        Ok(req) => {
            let headers = [("X-API-Key", api_key)];
            let mut builder = req
                .content_type(ContentType::ApplicationJson)
                .headers(&headers)
                .body(body.as_bytes());
            let result = builder.send(&mut rx_buf).await;
            match result {
                Ok(r) => {
                    log_runtime_line(format!("POST -> HTTP {}", r.status.0).as_str());
                    true
                }
                Err(e) => {
                    log_runtime_line(format!("POST error: {:?}", e).as_str());
                    false
                }
            }
        }
        Err(e) => {
            log_runtime_line(format!("HTTP connect error: {:?}", e).as_str());
            false
        }
    }
}
