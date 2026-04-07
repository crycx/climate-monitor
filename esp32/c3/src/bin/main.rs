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
    var MAX_SLOTS = 5;
    var cachedNetworks = [];
    var originalPasswords = {}; // slot index -> original password string

    function ssidId(i) { return i === 1 ? 'wifi_ssid' : 'wifi_ssid_' + i; }
    function passId(i) { return i === 1 ? 'wifi_password' : 'wifi_password_' + i; }
    function getVal(id) { var el = document.getElementById(id); return el ? el.value : ''; }
    function setVal(id, v) { var el = document.getElementById(id); if (el) el.value = v; }

    function findFieldsets() {
        var fss = {};
        document.querySelectorAll('fieldset').forEach(function (fs) {
            var leg = fs.querySelector('legend');
            if (leg) fss[leg.textContent.trim()] = fs;
        });
        return fss;
    }

    function hideWifiFieldsets(fss) {
        var labels = ['WiFi'];
        for (var i = 2; i <= MAX_SLOTS; i++) labels.push('WiFi slot ' + i);
        labels.forEach(function (l) { if (fss[l]) fss[l].style.display = 'none'; });
    }

    /* ── Scan ── */
    function doScan(btn, statusEl, selects) {
        btn.disabled = true;
        if (statusEl) statusEl.textContent = 'Scanning...';
        fetch('/wifi-scan')
            .then(function (r) { return r.ok ? r.json() : { networks: [] }; })
            .then(function (d) {
                cachedNetworks = (d.networks || []).filter(function (n) { return n && n.ssid; });
                selects.forEach(function (sel) { fillSelect(sel, cachedNetworks); });
                if (statusEl) statusEl.textContent = cachedNetworks.length
                    ? 'Found ' + cachedNetworks.length + ' network(s).'
                    : 'No networks found.';
            })
            .catch(function () { if (statusEl) statusEl.textContent = 'Scan failed.'; })
            .finally(function () { btn.disabled = false; });
    }

    function fillSelect(sel, networks) {
        var cur = sel.value;
        sel.innerHTML = '<option value="">\u2014 select \u2014</option>';
        networks.forEach(function (n) {
            var o = document.createElement('option');
            o.value = n.ssid;
            o.textContent = n.ssid + ' (' + n.rssi + ' dBm)';
            sel.appendChild(o);
        });
        sel.value = cur;
    }

    function makeScanRow(ssidInput) {
        var wrap = document.createElement('div');
        wrap.style.cssText = 'margin-top:8px';

        var row = document.createElement('div');
        row.style.cssText = 'display:flex;gap:6px;align-items:center';

        var btn = document.createElement('button');
        btn.type = 'button';
        btn.textContent = 'Scan WiFi';
        btn.style.cssText = 'padding:4px 10px;font-size:13px;cursor:pointer;flex-shrink:0';

        var sel = document.createElement('select');
        sel.style.cssText = 'flex:1;font-size:13px';
        if (cachedNetworks.length) fillSelect(sel, cachedNetworks);

        var statusEl = document.createElement('div');
        statusEl.style.cssText = 'font-size:12px;color:#666;margin-top:4px';

        sel.addEventListener('change', function () { if (sel.value) ssidInput.value = sel.value; });
        btn.addEventListener('click', function () { doScan(btn, statusEl, [sel]); });

        row.appendChild(btn);
        row.appendChild(sel);
        wrap.appendChild(row);
        wrap.appendChild(statusEl);
        return wrap;
    }

    /* ── Network card ── */
    function makeCard(slot, ssid, isNew) {
        var card = document.createElement('div');
        card.dataset.wifiSlot = slot;
        card.style.cssText = 'background:#f8f9fa;border:1px solid #ddd;border-radius:8px;padding:12px 14px;margin-bottom:8px';

        /* SSID row */
        var ssidRow = document.createElement('div');
        ssidRow.style.cssText = 'display:flex;gap:8px;align-items:center;margin-bottom:8px';

        var ssidLbl = document.createElement('label');
        ssidLbl.textContent = 'SSID';
        ssidLbl.style.cssText = 'width:78px;font-size:13px;font-weight:600;flex-shrink:0';

        var ssidInput = document.createElement('input');
        ssidInput.type = 'text';
        ssidInput.value = ssid;
        ssidInput.style.cssText = 'flex:1;padding:5px 8px;font-size:13px;border:1px solid #ccc;border-radius:4px';
        ssidInput.addEventListener('input', function () { setVal(ssidId(slot), ssidInput.value); });

        var delBtn = document.createElement('button');
        delBtn.type = 'button';
        delBtn.textContent = '\u2715';
        delBtn.title = 'Remove this network';
        delBtn.style.cssText = 'padding:4px 9px;background:#dc3545;color:#fff;border:none;border-radius:4px;cursor:pointer;font-size:14px;flex-shrink:0';
        delBtn.addEventListener('click', function () {
            setVal(ssidId(slot), '');
            setVal(passId(slot), '');
            originalPasswords[slot] = '';
            renderList();
        });

        ssidRow.appendChild(ssidLbl);
        ssidRow.appendChild(ssidInput);
        ssidRow.appendChild(delBtn);
        card.appendChild(ssidRow);

        /* Password row */
        var passRow = document.createElement('div');
        passRow.style.cssText = 'display:flex;gap:8px;align-items:center';

        var passLbl = document.createElement('label');
        passLbl.textContent = 'Password';
        passLbl.style.cssText = 'width:78px;font-size:13px;font-weight:600;flex-shrink:0';

        var passInput = document.createElement('input');
        passInput.type = 'password';
        var origPass = originalPasswords[slot] || '';
        passInput.placeholder = (!isNew && origPass) ? '(unchanged)' : '';
        passInput.style.cssText = 'flex:1;padding:5px 8px;font-size:13px;border:1px solid #ccc;border-radius:4px';

        /* No-password checkbox */
        var noPassWrap = document.createElement('label');
        noPassWrap.style.cssText = 'display:flex;align-items:center;gap:5px;font-size:13px;white-space:nowrap;flex-shrink:0;cursor:pointer';
        var noPassChk = document.createElement('input');
        noPassChk.type = 'checkbox';
        // Open network = saved SSID but password was empty string
        noPassChk.checked = !isNew && ssid && origPass === '';
        noPassChk.addEventListener('change', function () {
            passInput.disabled = noPassChk.checked;
            if (noPassChk.checked) passInput.value = '';
        });
        if (noPassChk.checked) passInput.disabled = true;
        noPassWrap.appendChild(noPassChk);
        noPassWrap.appendChild(document.createTextNode('No password'));

        passRow.appendChild(passLbl);
        passRow.appendChild(passInput);
        passRow.appendChild(noPassWrap);
        card.appendChild(passRow);

        /* Scan row only for new networks */
        if (isNew) card.appendChild(makeScanRow(ssidInput));

        /* Expose refs for pre-save sync */
        card._passInput = passInput;
        card._noPassChk = noPassChk;
        card._slot = slot;

        return card;
    }

    /* ── List renderer ── */
    var listEl, addBtn;

    function renderList() {
        listEl.innerHTML = '';
        var count = 0;
        for (var i = 1; i <= MAX_SLOTS; i++) {
            var ssid = getVal(ssidId(i));
            if (ssid) { listEl.appendChild(makeCard(i, ssid, false)); count++; }
        }
        if (count === 0) {
            var empty = document.createElement('p');
            empty.textContent = 'No networks saved yet. Click \u201c+ Add network\u201d to add one.';
            empty.style.cssText = 'color:#888;font-size:14px;margin:4px 0 12px';
            listEl.appendChild(empty);
        }
        addBtn.style.display = count >= MAX_SLOTS ? 'none' : '';
    }

    /* ── Manager widget ── */
    function buildManager() {
        var wrap = document.createElement('div');

        var h3 = document.createElement('h3');
        h3.textContent = 'WiFi Networks';
        h3.style.cssText = 'margin:0 0 10px;font-size:16px';
        wrap.appendChild(h3);

        listEl = document.createElement('div');
        wrap.appendChild(listEl);

        addBtn = document.createElement('button');
        addBtn.type = 'button';
        addBtn.textContent = '+ Add network';
        addBtn.style.cssText = 'padding:6px 14px;cursor:pointer;font-size:14px';
        addBtn.addEventListener('click', function () {
            for (var i = 1; i <= MAX_SLOTS; i++) {
                if (!getVal(ssidId(i))) {
                    // Remove "no networks" message
                    var empty = listEl.querySelector('p');
                    if (empty) listEl.removeChild(empty);
                    listEl.appendChild(makeCard(i, '', true));
                    var count = listEl.querySelectorAll('[data-wifi-slot]').length;
                    addBtn.style.display = count >= MAX_SLOTS ? 'none' : '';
                    break;
                }
            }
        });
        wrap.appendChild(addBtn);
        return wrap;
    }

    /* ── Pre-save: sync password fields from card state ── */
    function hookSave() {
        var form = document.querySelector('form');
        if (!form) return;
        form.addEventListener('submit', function () {
            listEl.querySelectorAll('[data-wifi-slot]').forEach(function (card) {
                var slot = card._slot;
                var pi = card._passInput;
                var nc = card._noPassChk;
                if (nc && nc.checked) {
                    setVal(passId(slot), '');
                } else if (pi && pi.value === '') {
                    // Empty input = keep original password
                    setVal(passId(slot), originalPasswords[slot] || '');
                } else if (pi) {
                    setVal(passId(slot), pi.value);
                }
            });
        }, true);
    }

    /* ── Install ── */
    function install() {
        if (document.getElementById('wifi-mgr')) return;

        // Snapshot original passwords before hiding
        for (var i = 1; i <= MAX_SLOTS; i++) {
            originalPasswords[i] = getVal(passId(i));
        }

        var fss = findFieldsets();
        var wifiFs = fss['WiFi'];
        if (!wifiFs) return;

        var mgr = buildManager();
        mgr.id = 'wifi-mgr';
        wifiFs.parentNode.insertBefore(mgr, wifiFs);
        hideWifiFieldsets(fss);
        renderList();
        hookSave();
    }

    var origLoad = window.loadConfig_main;
    if (typeof origLoad === 'function') {
        window.loadConfig_main = function () {
            return Promise.resolve(origLoad()).then(function (r) { install(); return r; });
        };
    }
    window.addEventListener('load', install);
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

    #[config_store(env_default = "WIFI_SSID_4", notify = "Wifi")]
    #[config_form(fieldset = "WiFi slot 4", label = "SSID", help = "Optional fourth network name")]
    wifi_ssid_4: String,

    #[config_store(env_default = "WIFI_PASSWORD_4", notify = "Wifi")]
    #[config_form(fieldset = "WiFi slot 4", label = "Password", input_type = "password", help = "Optional fourth network password")]
    wifi_password_4: String,

    #[config_store(env_default = "WIFI_SSID_5", notify = "Wifi")]
    #[config_form(fieldset = "WiFi slot 5", label = "SSID", help = "Optional fifth network name")]
    wifi_ssid_5: String,

    #[config_store(env_default = "WIFI_PASSWORD_5", notify = "Wifi")]
    #[config_form(fieldset = "WiFi slot 5", label = "Password", input_type = "password", help = "Optional fifth network password")]
    wifi_password_5: String,

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
        (cfg.wifi_ssid_4.as_str(), cfg.wifi_password_4.as_str()),
        (cfg.wifi_ssid_5.as_str(), cfg.wifi_password_5.as_str()),
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

    // Startup: scan first to pick the best visible network.
    {
        let has_config = {
            let cfg = config_handle.config().lock().await;
            !cfg.wifi_ssid.is_empty()
                || !cfg.wifi_ssid_2.is_empty()
                || !cfg.wifi_ssid_3.is_empty()
                || !cfg.wifi_ssid_4.is_empty()
                || !cfg.wifi_ssid_5.is_empty()
        };

        if has_config {
            // Start AP briefly so the caddy runner can scan visible networks.
            AP_ACTIVE.store(true, Ordering::Relaxed);
            wifi_sender.send(WifiCaddyCommand::APUp("ClimateMonitor-".into())).await;
            log_runtime_line("Boot: scanning for visible networks (5 s)...");
            Timer::after(Duration::from_secs(5)).await;

            // find_best_ssid now has scan results: prefers visible slots, falls back to slot 1.
            let best = {
                let cfg = config_handle.config().lock().await;
                find_best_ssid(&cfg)
            };

            if let Some((ssid, pass)) = best {
                log_runtime_line(format!("Boot scan: connecting to {}", ssid).as_str());
                wifi_sender.send(WifiCaddyCommand::StaUp(ssid, pass)).await;
                AP_ACTIVE.store(false, Ordering::Relaxed);
                wifi_sender.send(WifiCaddyCommand::APDown).await;
            }
        } else {
            log_runtime_line("No WiFi config: starting AP portal at 192.168.4.1");
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
        // Wait up to 30 s OR react immediately when config is saved in the portal.
        let config_changed = embassy_time::with_timeout(
            Duration::from_secs(30),
            config_rx.receive(),
        )
        .await
        .is_ok();

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

        if config_changed {
            // Scan results are fresh (AP was up during portal use).
            let best = {
                let cfg = config_handle.config().lock().await;
                find_best_ssid(&cfg)
            };
            if let Some((ssid, pass)) = best {
                log_runtime_line(format!("Config saved: connecting to {}", ssid).as_str());
                wifi_sender.send(WifiCaddyCommand::StaUp(ssid, pass)).await;
            }
            if AP_ACTIVE.load(Ordering::Relaxed) {
                AP_ACTIVE.store(false, Ordering::Relaxed);
                wifi_sender.send(WifiCaddyCommand::APDown).await;
                log_runtime_line("AP portal closed, returning to normal mode.");
            }
            continue; // skip sensor read this cycle
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
