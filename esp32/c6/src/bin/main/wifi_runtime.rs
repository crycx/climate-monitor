extern crate alloc;

use alloc::string::String;
use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, Ordering};

use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_net::{Ipv4Address, Ipv4Cidr, Runner, Stack, StackResources, StaticConfigV4};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, Receiver, Sender};
use embassy_time::{Duration, Instant, Timer, with_timeout};
use esp_hal::peripherals::WIFI;
use esp_hal::rng::Rng;
use esp_radio::wifi::ap::{AccessPointConfig, AccessPointInfo};
use esp_radio::wifi::scan::ScanConfig;
use esp_radio::wifi::sta::StationConfig;
use esp_radio::wifi::{
    AuthenticationMethod, Config as ModeConfig, ControllerConfig, Interface as WifiDevice,
    WifiController,
};
use log::{debug, error, info, warn};

macro_rules! mk_static {
    ($t:ty, $val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        STATIC_CELL.uninit().write($val)
    }};
}

#[derive(Debug, Clone)]
pub enum WifiCommand {
    APUp(String),
    APDown,
    StaUp(String, String),
    StaDown,
    ScanOnce,
}

pub const WIFI_COMMAND_CHANNEL_CAPACITY: usize = 4;

pub type WifiCommandSender =
    Sender<'static, CriticalSectionRawMutex, WifiCommand, WIFI_COMMAND_CHANNEL_CAPACITY>;
type WifiCommandReceiver =
    Receiver<'static, CriticalSectionRawMutex, WifiCommand, WIFI_COMMAND_CHANNEL_CAPACITY>;

static WIFI_COMMAND_CHANNEL: Channel<
    CriticalSectionRawMutex,
    WifiCommand,
    WIFI_COMMAND_CHANNEL_CAPACITY,
> = Channel::new();

static SCAN_RESULTS_JSON: critical_section::Mutex<RefCell<Option<String>>> =
    critical_section::Mutex::new(RefCell::new(None));
static SCAN_SSIDS_FLAT: critical_section::Mutex<RefCell<Option<String>>> =
    critical_section::Mutex::new(RefCell::new(None));
static PORTAL_LOG_TEXT: critical_section::Mutex<RefCell<String>> =
    critical_section::Mutex::new(RefCell::new(String::new()));
static SCAN_IN_PROGRESS: AtomicBool = AtomicBool::new(false);
static STA_CONNECTED: AtomicBool = AtomicBool::new(false);

pub struct WifiStacks {
    pub sta: Stack<'static>,
    pub ap: Stack<'static>,
    /// ESP-NOW runs on the same radio as STA and receives on the STA channel.
    pub esp_now: esp_radio::esp_now::EspNow<'static>,
    pub sta_mac: [u8; 6],
}

const STA_RECONNECT_DELAY_MS: u64 = 5000;
const AP_SCAN_REFRESH_MS: u64 = 5000;
const PORTAL_LOG_MAX_CHARS: usize = 3000;
const AP_DEFAULT_CHANNEL: u8 = 11;
const AP_MAX_CONNECTIONS: u16 = 1;

fn portal_serial_log(line: &str) {
    esp_println::println!("{}", line);
    portal_log(line);
}

pub async fn init(spawner: &Spawner, wifi: WIFI<'static>) -> (WifiStacks, WifiCommandSender) {
    portal_log_clear();
    portal_log("wifi: init");

    let wifi_commands = WIFI_COMMAND_CHANNEL.receiver();
    let sender = WIFI_COMMAND_CHANNEL.sender();
    let sta_config: embassy_net::Config = embassy_net::Config::dhcpv4(Default::default());
    let ap_config = embassy_net::Config::ipv4_static(StaticConfigV4 {
        address: Ipv4Cidr::new(Ipv4Address::new(192, 168, 4, 1), 24),
        gateway: Some(Ipv4Address::new(192, 168, 4, 1)),
        dns_servers: Default::default(),
    });

    let rng = Rng::new();
    let seed = (rng.random() as u64) << 32 | rng.random() as u64;

    let wifi_config = ControllerConfig::default();
    let (controller, interfaces) = esp_radio::wifi::new(wifi, wifi_config).unwrap();
    let ap_interface = interfaces.access_point;
    let sta_interface = interfaces.station;
    let esp_now = interfaces.esp_now;

    let ap_mac: [u8; 6] = ap_interface.mac_address().try_into().unwrap();
    let sta_mac: [u8; 6] = sta_interface.mac_address().try_into().unwrap();
    let (sta_stack, sta_runner) = embassy_net::new(
        sta_interface,
        sta_config,
        mk_static!(StackResources<10>, StackResources::<10>::new()),
        seed,
    );
    let (ap_stack, ap_runner) = embassy_net::new(
        ap_interface,
        ap_config,
        mk_static!(StackResources<10>, StackResources::<10>::new()),
        seed,
    );

    spawner.spawn(connection(controller, ap_mac, wifi_commands).unwrap());
    spawner.spawn(ap_task(ap_runner).unwrap());
    spawner.spawn(sta_task(sta_runner).unwrap());

    (
        WifiStacks {
            sta: sta_stack,
            ap: ap_stack,
            esp_now,
            sta_mac,
        },
        sender,
    )
}

pub async fn send_command(cmd: WifiCommand) {
    WIFI_COMMAND_CHANNEL.sender().send(cmd).await;
}

pub fn scan_results_flat() -> Option<String> {
    critical_section::with(|cs| SCAN_SSIDS_FLAT.borrow_ref(cs).clone())
}

pub fn has_scan_results() -> bool {
    critical_section::with(|cs| {
        SCAN_SSIDS_FLAT
            .borrow_ref(cs)
            .as_deref()
            .map(|flat| !flat.is_empty())
            .unwrap_or(false)
    })
}

pub fn scan_ssid_visible(ssid: &str) -> bool {
    if ssid.is_empty() {
        return false;
    }
    critical_section::with(|cs| {
        SCAN_SSIDS_FLAT
            .borrow_ref(cs)
            .as_deref()
            .map(|flat| flat.lines().any(|line| line == ssid))
            .unwrap_or(false)
    })
}

pub fn scan_results_json() -> String {
    critical_section::with(|cs| {
        SCAN_RESULTS_JSON
            .borrow_ref(cs)
            .clone()
            .unwrap_or_else(|| String::from("{\"networks\":[]}"))
    })
}

pub fn portal_log(line: &str) {
    critical_section::with(|cs| {
        let mut text = PORTAL_LOG_TEXT.borrow_ref_mut(cs);
        let incoming_len = line.len().saturating_add(1);
        if text.len().saturating_add(incoming_len) > PORTAL_LOG_MAX_CHARS {
            let to_drop = text
                .len()
                .saturating_add(incoming_len)
                .saturating_sub(PORTAL_LOG_MAX_CHARS);
            if to_drop > 0 {
                if let Some(next_line) = text[to_drop.min(text.len())..].find('\n') {
                    let cut_from = to_drop.min(text.len()).saturating_add(next_line + 1);
                    let text_len = text.len();
                    text.drain(..cut_from.min(text_len));
                } else {
                    text.clear();
                }
            }
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(line);
    });
}

pub fn portal_log_text() -> String {
    critical_section::with(|cs| PORTAL_LOG_TEXT.borrow_ref(cs).clone())
}

pub fn portal_log_clear() {
    critical_section::with(|cs| PORTAL_LOG_TEXT.borrow_ref_mut(cs).clear());
}

pub fn scan_in_progress() -> bool {
    SCAN_IN_PROGRESS.load(Ordering::Relaxed)
}

pub fn sta_connected() -> bool {
    STA_CONNECTED.load(Ordering::Relaxed)
}

fn json_escape(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len() + 8);
    for ch in input.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn store_scan_results(networks: &[AccessPointInfo]) {
    let mut body = String::from("{\"networks\":[");
    let mut first = true;
    for network in networks.iter().take(24) {
        if network.ssid.is_empty() {
            continue;
        }
        if !first {
            body.push(',');
        }
        first = false;
        body.push_str("{\"ssid\":\"");
        body.push_str(&json_escape(network.ssid.as_str()));
        body.push_str("\",\"rssi\":");
        body.push_str(alloc::format!("{}", network.signal_strength).as_str());
        body.push('}');
    }
    body.push_str("]}");

    let mut flat = String::new();
    for network in networks.iter().take(24) {
        if network.ssid.is_empty() {
            continue;
        }
        if !flat.is_empty() {
            flat.push('\n');
        }
        flat.push_str(network.ssid.as_str());
    }

    critical_section::with(|cs| {
        *SCAN_RESULTS_JSON.borrow_ref_mut(cs) = Some(body);
        *SCAN_SSIDS_FLAT.borrow_ref_mut(cs) = Some(flat);
    });
}

fn clear_scan_results() {
    critical_section::with(|cs| {
        *SCAN_RESULTS_JSON.borrow_ref_mut(cs) = Some(String::from("{\"networks\":[]}"));
        *SCAN_SSIDS_FLAT.borrow_ref_mut(cs) = Some(String::new());
    });
}

struct WifiRunner {
    controller: WifiController<'static>,
    ap_up: bool,
    ap_ssid_prefix: String,
    ap_mac: [u8; 6],
    ssid: String,
    pass: String,
    wifi_commands: WifiCommandReceiver,
    last_scan_attempt: Option<Instant>,
    started_mode: u8,
    scan_refresh_enabled: bool,
}

fn mode_variant(config: &ModeConfig) -> u8 {
    match config {
        ModeConfig::Station(_) => 1,
        ModeConfig::AccessPoint(_) => 2,
        ModeConfig::AccessPointStation(_, _) => 3,
        #[cfg(feature = "wifi-eap")]
        ModeConfig::EapStation(_) => 4,
        _ => 4,
    }
}

impl WifiRunner {
    fn new(
        controller: WifiController<'static>,
        ap_mac: [u8; 6],
        wifi_commands: WifiCommandReceiver,
    ) -> Self {
        Self {
            controller,
            ap_up: false,
            ap_ssid_prefix: String::new(),
            ap_mac,
            ssid: String::new(),
            pass: String::new(),
            wifi_commands,
            last_scan_attempt: None,
            started_mode: 0,
            scan_refresh_enabled: false,
        }
    }

    fn ap_ssid(&self) -> String {
        let secure_tag = if Self::ap_password().is_some() {
            "sec-"
        } else {
            ""
        };
        alloc::fmt::format(format_args!(
            "{}{}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            self.ap_ssid_prefix,
            secure_tag,
            self.ap_mac[0],
            self.ap_mac[1],
            self.ap_mac[2],
            self.ap_mac[3],
            self.ap_mac[4],
            self.ap_mac[5]
        ))
    }

    fn ap_channel() -> u8 {
        option_env!("AP_CHANNEL")
            .and_then(|raw| raw.parse::<u8>().ok())
            .filter(|ch| (1..=11).contains(ch))
            .unwrap_or(AP_DEFAULT_CHANNEL)
    }

    fn ap_password() -> Option<String> {
        option_env!("AP_PORTAL_PASSWORD")
            .filter(|raw| raw.len() >= 8)
            .map(String::from)
    }

    fn build_ap_config(&self, ap_only: bool) -> AccessPointConfig {
        let mut cfg = AccessPointConfig::default()
            .with_ssid(self.ap_ssid())
            .with_password("".into())
            .with_max_connections(AP_MAX_CONNECTIONS);

        if let Some(password) = Self::ap_password() {
            cfg = cfg
                .with_auth_method(AuthenticationMethod::Wpa2Personal)
                .with_password(password);
        }

        if ap_only {
            cfg.with_channel(Self::ap_channel())
        } else {
            // In AP+STA mode, let the radio pick the compatible channel with STA.
            cfg
        }
    }

    fn current_config(&self) -> Option<ModeConfig> {
        if !self.ap_up && self.ssid.is_empty() {
            None
        } else if !self.ap_up && !self.ssid.is_empty() {
            Some(ModeConfig::Station(
                StationConfig::default()
                    .with_ssid(self.ssid.clone())
                    .with_password(self.pass.clone()),
            ))
        } else if self.ap_up && self.ssid.is_empty() {
            Some(ModeConfig::AccessPoint(self.build_ap_config(true)))
        } else {
            Some(ModeConfig::AccessPointStation(
                StationConfig::default()
                    .with_ssid(self.ssid.clone())
                    .with_password(self.pass.clone()),
                self.build_ap_config(false),
            ))
        }
    }

    fn apply_config(&mut self, config: &ModeConfig) {
        if let Err(e) = self.controller.set_config(config) {
            warn!("wifi: failed to set config: {:?}", e);
        }
    }

    async fn ensure_wifi_started_with_config(&mut self, config: Option<&ModeConfig>) {
        if config.is_none() {
            self.started_mode = 0;
            STA_CONNECTED.store(false, Ordering::Relaxed);
            return;
        }

        let config = config.unwrap();
        let new_variant = mode_variant(config);
        if self.started_mode != new_variant {
            debug!("wifi: applying mode change");
        }
        self.apply_config(config);
        self.started_mode = new_variant;
    }

    async fn try_connect_sta(&mut self) {
        if self.ssid.is_empty() {
            return;
        }
        if self.controller.is_connected() {
            return;
        }
        if self.ap_up && self.ssid.is_empty() {
            debug!("wifi: AP-only mode, skipping STA reconnect attempt");
            return;
        }

        debug!("wifi: connecting to STA");
        // connect_async() blocks until StaConnected or StaDisconnected event.
        // If neither fires (wifi stack in bad state after scan stop/start), it hangs forever
        // and the runner can never process further commands (StaDown, StaUp etc).
        // Timeout prevents permanent deadlock.
        match with_timeout(Duration::from_secs(20), self.controller.connect_async()).await {
            Ok(Ok(_)) => debug!("wifi: STA connected"),
            Ok(Err(e)) => {
                error!("wifi: STA connect failed: {:?}", e);
                Timer::after(Duration::from_millis(STA_RECONNECT_DELAY_MS)).await;
            }
            Err(_) => {
                error!("wifi: STA connect timed out - no wifi event received; retrying");
                Timer::after(Duration::from_millis(STA_RECONNECT_DELAY_MS)).await;
            }
        }
    }

    async fn sync_state(&mut self) {
        let config = self.current_config();
        debug!("wifi: sync_state -> {:?}", config);
        self.ensure_wifi_started_with_config(config.as_ref()).await;
        self.try_connect_sta().await;
    }

    async fn refresh_scan_results(&mut self) {
        if !self.scan_refresh_enabled || !self.ap_up {
            return;
        }

        let now = Instant::now();
        if let Some(last) = self.last_scan_attempt {
            if now.duration_since(last) < Duration::from_secs(3) {
                return;
            }
        } else {
            Timer::after(Duration::from_secs(1)).await;
        }
        self.last_scan_attempt = Some(now);

        let config = ScanConfig::default().with_show_hidden(false).with_max(32);
        match self.controller.scan_async(&config).await {
            Ok(results) => {
                info!("wifi: cached {} visible networks", results.len());
                portal_log(alloc::format!("scan: {} networks", results.len()).as_str());
                store_scan_results(&results);
            }
            Err(e) => {
                warn!("wifi: scan failed: {:?}", e);
                portal_log(alloc::format!("scan: failed ({:?})", e).as_str());
            }
        }
    }

    async fn do_scan_once(&mut self) {
        if self.ap_up && self.ssid.is_empty() {
            portal_serial_log("scan: skipped in AP-only mode (using cached results)");
            return;
        }

        let was_started = self.started_mode != 0;
        let restore_to_none = self.current_config().is_none();

        if !was_started {
            if restore_to_none {
                info!("wifi: scan_once starting temporary STA mode");
                portal_log("scan: starting temporary STA mode");
                let scan_mode = ModeConfig::Station(StationConfig::default());
                self.ensure_wifi_started_with_config(Some(&scan_mode)).await;
            } else {
                info!("wifi: scan_once restoring wifi state before scan");
                portal_log("scan: restoring wifi state before scan");
                self.sync_state().await;
            }
        }

        SCAN_IN_PROGRESS.store(true, Ordering::Relaxed);
        let config = ScanConfig::default().with_show_hidden(false).with_max(32);
        match self.controller.scan_async(&config).await {
            Ok(results) => {
                info!("wifi: scan_once: {} networks found", results.len());
                portal_log(alloc::format!("scan: found {} network(s)", results.len()).as_str());
                store_scan_results(&results);
            }
            Err(e) => {
                warn!("wifi: scan_once failed: {:?}", e);
                portal_log(alloc::format!("scan: failed ({:?})", e).as_str());
                clear_scan_results();
            }
        }
        SCAN_IN_PROGRESS.store(false, Ordering::Relaxed);

        if !was_started {
            if restore_to_none {
                self.ensure_wifi_started_with_config(None).await;
                info!("wifi: scan_once stopped temporary STA mode");
                portal_log("scan: temporary STA mode stopped");
            } else {
                self.sync_state().await;
            }
        }
    }

    async fn handle_command(&mut self, cmd: WifiCommand) {
        match cmd {
            WifiCommand::APUp(prefix) => {
                info!("wifi: APUp command");
                self.ap_ssid_prefix = prefix;
                self.ap_up = true;
                let ap_ssid = self.ap_ssid();
                let ap_mode = if self.ssid.is_empty() { "AP" } else { "AP+STA" };
                let ap_channel = if self.ssid.is_empty() {
                    Self::ap_channel()
                } else {
                    0
                };
                info!("wifi: AP SSID {}", ap_ssid.as_str());
                let ap_security = if Self::ap_password().is_some() {
                    "WPA2"
                } else {
                    "OPEN"
                };
                if ap_channel == 0 {
                    portal_serial_log(
                        alloc::format!(
                            "ap: on, mode {}, security {}, ssid '{}', channel auto-with-sta, max_conn {}, ip 192.168.4.1",
                            ap_mode,
                            ap_security,
                            ap_ssid,
                            AP_MAX_CONNECTIONS
                        )
                        .as_str(),
                    );
                } else {
                    portal_serial_log(
                        alloc::format!(
                            "ap: on, mode {}, security {}, ssid '{}', channel {}, max_conn {}, ip 192.168.4.1",
                            ap_mode,
                            ap_security,
                            ap_ssid,
                            ap_channel,
                            AP_MAX_CONNECTIONS
                        )
                        .as_str(),
                    );
                }
            }
            WifiCommand::APDown => {
                info!("wifi: APDown command");
                self.ap_up = false;
                clear_scan_results();
                portal_serial_log("ap: off");
            }
            WifiCommand::StaUp(new_ssid, new_pass) => {
                info!("wifi: StaUp command");
                portal_serial_log(alloc::format!("sta: set ssid '{}'", new_ssid).as_str());
                self.ssid = new_ssid;
                self.pass = new_pass;
            }
            WifiCommand::StaDown => {
                info!("wifi: StaDown command");
                portal_serial_log("sta: off");
                self.ssid.clear();
                self.pass.clear();
            }
            WifiCommand::ScanOnce => {
                info!("wifi: ScanOnce command");
                self.do_scan_once().await;
            }
        }
    }

    async fn run(&mut self) {
        self.sync_state().await;
        self.refresh_scan_results().await;

        loop {
            match select(
                self.wifi_commands.receive(),
                Timer::after(Duration::from_millis(AP_SCAN_REFRESH_MS)),
            )
            .await {
                Either::First(cmd) => {
                    let should_sync = !matches!(cmd, WifiCommand::ScanOnce);
                    self.handle_command(cmd).await;
                    if should_sync {
                        self.sync_state().await;
                    }
                    self.refresh_scan_results().await;
                }
                Either::Second(_) => {
                    // Keep STA link alive and refresh scan cache while AP is open.
                    self.try_connect_sta().await;
                    self.refresh_scan_results().await;
                }
            }
        }
    }
}

#[embassy_executor::task]
async fn connection(
    controller: WifiController<'static>,
    ap_mac: [u8; 6],
    wifi_commands: WifiCommandReceiver,
) {
    let mut runner = WifiRunner::new(controller, ap_mac, wifi_commands);
    runner.run().await;
}

#[embassy_executor::task]
async fn sta_task(mut runner: Runner<'static, WifiDevice<'static>>) {
    info!("start STA task");
    runner.run().await;
}

#[embassy_executor::task]
async fn ap_task(mut runner: Runner<'static, WifiDevice<'static>>) {
    info!("start AP task");
    runner.run().await;
}

pub fn set_scan_results_from_flat(flat: &str) {
    let mut body = String::from("{\"networks\":[");
    let mut first = true;
    for ssid in flat.lines().filter(|line| !line.is_empty()).take(24) {
        if !first {
            body.push(',');
        }
        first = false;
        body.push_str("{\"ssid\":\"");
        body.push_str(&json_escape(ssid));
        body.push_str("\",\"rssi\":0}");
    }
    body.push_str("]}");

    let flat_owned = String::from(flat);
    critical_section::with(|cs| {
        *SCAN_RESULTS_JSON.borrow_ref_mut(cs) = Some(body);
        *SCAN_SSIDS_FLAT.borrow_ref_mut(cs) = Some(flat_owned);
    });
}