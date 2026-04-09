extern crate alloc;

use alloc::{format, string::String};
use core::fmt::{Debug, Display};
use core::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use edge_dhcp::io::DEFAULT_SERVER_PORT;
use edge_dhcp::server::{Server as DhcpServer, ServerOptions};
use edge_http::io::Error;
use edge_http::io::server::{Connection, Handler, Server};
use edge_nal::{TcpBind, TcpSplit, UdpBind};
use edge_nal_embassy::{Tcp, TcpBuffers, Udp, UdpBuffers};
use embassy_futures::select::{Either, select};
use embassy_net::Stack;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, Receiver};
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Timer};
use embedded_io_async::{ErrorType, Read, Write};
use log::{error, info};

use crate::app_config::{AppConfig, FlashConfigStorage};
use crate::wifi_runtime::{self, WifiCommand};

const AP_IP_ADDRESS: Ipv4Addr = Ipv4Addr::new(192, 168, 4, 1);
const AP_DNS_ADDRESS: Ipv4Addr = Ipv4Addr::new(192, 168, 4, 1);
const AP_POOL_START: Ipv4Addr = Ipv4Addr::new(192, 168, 4, 100);
const AP_POOL_END: Ipv4Addr = Ipv4Addr::new(192, 168, 4, 200);
const HANDLER_TASKS: usize = 4;
const TCP_BUF_SIZE: usize = 2048;
const HTTP_BUF_SIZE: usize = 8192;
const KEEPALIVE_TIMEOUT_MS: u32 = 10000;
static CONFIG_UPDATE_CHANNEL: Channel<CriticalSectionRawMutex, bool, 2> = Channel::new();

pub type ConfigUpdateReceiver = Receiver<'static, CriticalSectionRawMutex, bool, 2>;

pub fn config_update_receiver() -> ConfigUpdateReceiver {
    CONFIG_UPDATE_CHANNEL.receiver()
}

pub fn start(
    spawner: embassy_executor::Spawner,
    ap_stack: Stack<'static>,
    config: &'static Mutex<CriticalSectionRawMutex, AppConfig>,
    storage: &'static Mutex<CriticalSectionRawMutex, FlashConfigStorage<'static>>,
) {
    spawner.spawn(dhcp_task(ap_stack)).unwrap();
    // DNS captive task is temporarily disabled; HTTP redirect endpoints remain enabled.
    // Re-enable after AP stability is fully verified on target devices.
    // spawner.spawn(dns_task(ap_stack)).unwrap();
    spawner
        .spawn(http_task(ap_stack, config, storage))
        .unwrap();
}

#[embassy_executor::task]
async fn dns_task(stack: Stack<'static>) {
    info!("dns: start DNS task");
    wifi_runtime::portal_log("dns: task started");

    let buffers = UdpBuffers::<1, 1500, 1500, 2>::new();
    let udp = Udp::new(stack, &buffers);
    let mut tx_buf = [0u8; 1500];
    let mut rx_buf = [0u8; 1500];

    loop {
        if !stack.is_link_up() {
            wifi_runtime::portal_log("dns: waiting for AP link up");
            stack.wait_link_up().await;
            wifi_runtime::portal_log("dns: AP link is up");
        }

        if let Err(_e) = edge_captive::io::run(
            &udp,
            edge_captive::io::DEFAULT_SOCKET,
            &mut tx_buf,
            &mut rx_buf,
            AP_IP_ADDRESS,
            Duration::from_secs(60).into(),
        )
        .await
        {
            error!("dns: server error, restarting");
            wifi_runtime::portal_log("dns: server error, restarting");
        }

        Timer::after(Duration::from_secs(1)).await;
    }
}

#[embassy_executor::task]
async fn http_task(
    stack: Stack<'static>,
    config: &'static Mutex<CriticalSectionRawMutex, AppConfig>,
    storage: &'static Mutex<CriticalSectionRawMutex, FlashConfigStorage<'static>>,
) {
    let handler = PortalHandler { config, storage };
    run_http_loop(stack, handler).await;
}

#[embassy_executor::task]
async fn dhcp_task(stack: Stack<'static>) {
    info!("dhcp: start DHCP task");
    wifi_runtime::portal_log("dhcp: task started");

    loop {
        if !stack.is_link_up() {
            info!("dhcp: waiting for AP link...");
            wifi_runtime::portal_log("dhcp: waiting for AP link up");
            stack.wait_link_up().await;
            info!("dhcp: AP link is up, starting DHCP server");
            wifi_runtime::portal_log("dhcp: AP link is up, starting DHCP server");
        }

        let buffers = UdpBuffers::<1, 1500, 1500, 2>::new();
        let udp = Udp::new(stack, &buffers);

        let mut socket = match udp
            .bind(SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::UNSPECIFIED,
                DEFAULT_SERVER_PORT,
            )))
            .await
        {
            Ok(s) => s,
            Err(_) => {
                error!("dhcp: bind error, retrying in 1 s");
                wifi_runtime::portal_log("dhcp: bind failed, retrying in 1 s");
                Timer::after(Duration::from_secs(1)).await;
                continue;
            }
        };

        wifi_runtime::portal_log("dhcp: listening on 0.0.0.0:67");

        let mut server: DhcpServer<_, 64> = DhcpServer::new_with_et(AP_IP_ADDRESS);
        server.range_start = AP_POOL_START;
        server.range_end = AP_POOL_END;

        let mut gw_buf = [Ipv4Addr::UNSPECIFIED];
        let dns = [AP_DNS_ADDRESS];
        let mut server_options = ServerOptions::new(AP_IP_ADDRESS, Some(&mut gw_buf));
        server_options.dns = &dns;

        let mut buf = [0; 1500];
        match select(
            edge_dhcp::io::server::run(&mut server, &server_options, &mut socket, &mut buf),
            stack.wait_link_down(),
        )
        .await
        {
            Either::First(Ok(())) => {}
            Either::First(Err(_)) => {
                error!("dhcp: server error, restarting in 1 s");
                wifi_runtime::portal_log("dhcp: server error, restarting");
            }
            Either::Second(_) => {
                info!("dhcp: AP link went down, restarting");
                wifi_runtime::portal_log("dhcp: AP link down, restarting");
            }
        }

        Timer::after(Duration::from_secs(1)).await;
    }
}

async fn run_http_loop<H: Handler>(stack: Stack<'static>, handler: H) {
    loop {
        if !stack.is_link_up() {
            info!("http: waiting for AP link...");
            wifi_runtime::portal_log("http: waiting for AP link up");
            stack.wait_link_up().await;
            info!("http: AP link is up");
            wifi_runtime::portal_log("http: AP link is up");
        }

        let tcp_buffers = TcpBuffers::<{ HANDLER_TASKS }, { TCP_BUF_SIZE }, { TCP_BUF_SIZE }>::new();
        let tcp = Tcp::new(stack, &tcp_buffers);
        let acceptor = loop {
            match tcp
                .bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 80)))
                .await
            {
                Ok(a) => {
                    info!("http: server ready at http://192.168.4.1/");
                    wifi_runtime::portal_log("http: ready at http://192.168.4.1/");
                    break a;
                }
                Err(_) => {
                    error!("http: bind failed, retrying in 200 ms");
                    wifi_runtime::portal_log("http: bind failed, retrying in 200 ms");
                    Timer::after(Duration::from_millis(200)).await;
                }
            }
        };

        let mut server = Server::<{ HANDLER_TASKS }, { HTTP_BUF_SIZE }>::new();
        match select(
            server.run(Some(KEEPALIVE_TIMEOUT_MS), acceptor, &handler),
            stack.wait_link_down(),
        )
        .await
        {
            Either::First(result) => {
                if let Err(e) = result {
                    error!("http: server error: {:?}", e);
                    wifi_runtime::portal_log("http: server error, task exiting");
                }
                return;
            }
            Either::Second(_) => {
                info!("http: AP link went down, rebinding after link-up...");
                wifi_runtime::portal_log("http: AP link down, rebinding after link up");
                Timer::after(Duration::from_millis(100)).await;
            }
        }
    }
}

struct PortalHandler {
    config: &'static Mutex<CriticalSectionRawMutex, AppConfig>,
    storage: &'static Mutex<CriticalSectionRawMutex, FlashConfigStorage<'static>>,
}

impl edge_http::io::server::Handler for PortalHandler {
    type Error<E: Debug> = Error<E>;

    async fn handle<T, const N: usize>(
        &self,
        task_id: impl Display + Copy,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), Self::Error<T::Error>>
    where
        T: Read + Write + TcpSplit,
    {
        let headers = conn.headers()?;
        let full_path = headers.path;
        let path = path_only(full_path);
        info!("http[{}]: {} {}", task_id, headers.method, full_path);

        match path {
            "/" => self.handle_root(conn, None).await,
            "/save" => self.handle_save(conn, full_path).await,
            "/wifi-scan" => self.handle_scan(conn).await,
            "/wifi-log" => send_text_string(
                conn,
                200,
                "text/plain; charset=utf-8",
                wifi_runtime::portal_log_text(),
            )
            .await,
            "/generate_204" | "/gen_204" | "/hotspot-detect.html" | "/ncsi.txt"
            | "/connecttest.txt" | "/redirect" | "/fwlink" => {
                send_redirect(conn, "http://192.168.4.1/").await
            }
            _ => send_text(conn, 404, "text/plain", "Not Found").await,
        }
    }
}

impl PortalHandler {
    async fn handle_root<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
        notice: Option<&str>,
    ) -> Result<(), Error<<T as ErrorType>::Error>>
    where
        T: Read + Write,
    {
        let cfg = self.config.lock().await.clone();
        send_setup_page(conn, &cfg, notice, false).await
    }

    async fn handle_scan<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), Error<<T as ErrorType>::Error>>
    where
        T: Read + Write,
    {
        wifi_runtime::send_command(WifiCommand::ScanOnce).await;
        Timer::after(Duration::from_millis(2500)).await;
        send_text_string(
            conn,
            200,
            "application/json",
            wifi_runtime::scan_results_json(),
        )
        .await
    }

    async fn handle_save<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
        full_path: &str,
    ) -> Result<(), Error<<T as ErrorType>::Error>>
    where
        T: Read + Write,
    {
        let previous = self.config.lock().await.clone();
        let next = apply_query_to_config(&previous, full_path);
        let changed = previous != next;

        if changed {
            {
                let mut cfg = self.config.lock().await;
                *cfg = next.clone();
            }
            {
                let mut storage = self.storage.lock().await;
                if let Err(e) = storage.store_config(&next).await {
                    error!("config: store failed: {}", e);
                    return send_text(conn, 500, "text/plain", "Config store failed").await;
                }
            }
        }

        // Always notify runtime on save so AP close/reconnect logic runs even if values are unchanged.
        CONFIG_UPDATE_CHANNEL.sender().send(changed).await;

        send_setup_page(
            conn,
            &next,
            Some("Settings saved. AP will close in 2 seconds."),
            true,
        )
        .await
    }
}

fn path_only(full: &str) -> &str {
    full.split_once('?').map_or(full, |(path, _)| path)
}

fn query_param(full_path: &str, key: &str) -> Option<String> {
    let query = full_path.split_once('?')?.1;
    for pair in query.split('&') {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        if name == key {
            return Some(percent_decode(value));
        }
    }
    None
}

fn query_checkbox(full_path: &str, key: &str) -> bool {
    query_param(full_path, key).is_some()
}

fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.as_bytes().iter();
    while let Some(&b) = chars.next() {
        if b == b'%' {
            let hi = chars.next().copied().unwrap_or(b'0');
            let lo = chars.next().copied().unwrap_or(b'0');
            let val = hex_nibble(hi) << 4 | hex_nibble(lo);
            out.push(val as char);
        } else if b == b'+' {
            out.push(' ');
        } else {
            out.push(b as char);
        }
    }
    out
}

fn hex_nibble(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => 0,
    }
}

fn apply_wifi_slot(
    old_ssid: &str,
    old_pass: &str,
    new_ssid: String,
    new_pass: String,
    no_password: bool,
) -> (String, String) {
    if new_ssid.is_empty() {
        return (String::new(), String::new());
    }
    if no_password {
        return (new_ssid, String::new());
    }
    if !new_pass.is_empty() {
        return (new_ssid, new_pass);
    }
    if old_ssid == new_ssid {
        return (new_ssid, old_pass.into());
    }
    (new_ssid, String::new())
}

fn apply_query_to_config(current: &AppConfig, full_path: &str) -> AppConfig {
    let mut next = current.clone();

    let (ssid, pass) = apply_wifi_slot(
        &current.wifi_ssid,
        &current.wifi_password,
        query_param(full_path, "wifi_ssid").unwrap_or_default(),
        query_param(full_path, "wifi_password").unwrap_or_default(),
        query_checkbox(full_path, "wifi_no_password_1"),
    );
    next.wifi_ssid = ssid;
    next.wifi_password = pass;

    let (ssid, pass) = apply_wifi_slot(
        &current.wifi_ssid_2,
        &current.wifi_password_2,
        query_param(full_path, "wifi_ssid_2").unwrap_or_default(),
        query_param(full_path, "wifi_password_2").unwrap_or_default(),
        query_checkbox(full_path, "wifi_no_password_2"),
    );
    next.wifi_ssid_2 = ssid;
    next.wifi_password_2 = pass;

    let (ssid, pass) = apply_wifi_slot(
        &current.wifi_ssid_3,
        &current.wifi_password_3,
        query_param(full_path, "wifi_ssid_3").unwrap_or_default(),
        query_param(full_path, "wifi_password_3").unwrap_or_default(),
        query_checkbox(full_path, "wifi_no_password_3"),
    );
    next.wifi_ssid_3 = ssid;
    next.wifi_password_3 = pass;

    let (ssid, pass) = apply_wifi_slot(
        &current.wifi_ssid_4,
        &current.wifi_password_4,
        query_param(full_path, "wifi_ssid_4").unwrap_or_default(),
        query_param(full_path, "wifi_password_4").unwrap_or_default(),
        query_checkbox(full_path, "wifi_no_password_4"),
    );
    next.wifi_ssid_4 = ssid;
    next.wifi_password_4 = pass;

    let (ssid, pass) = apply_wifi_slot(
        &current.wifi_ssid_5,
        &current.wifi_password_5,
        query_param(full_path, "wifi_ssid_5").unwrap_or_default(),
        query_param(full_path, "wifi_password_5").unwrap_or_default(),
        query_checkbox(full_path, "wifi_no_password_5"),
    );
    next.wifi_ssid_5 = ssid;
    next.wifi_password_5 = pass;

    next.api_url = query_param(full_path, "api_url").unwrap_or_default();
    next.api_key = query_param(full_path, "api_key").unwrap_or_default();
    next.temp_offset = query_param(full_path, "temp_offset").unwrap_or_else(|| current.temp_offset.clone());
    next.hum_offset = query_param(full_path, "hum_offset").unwrap_or_else(|| current.hum_offset.clone());
    next
}

fn html_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 8);
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

fn render_wifi_slot(index: usize, ssid: &str, password: &str, visible: bool) -> String {
    let suffix = if index == 1 {
        String::new()
    } else {
        format!("_{}", index)
    };
    let checked = if ssid.is_empty() {
        ""
    } else if password.is_empty() {
        "checked"
    } else {
        ""
    };
    let placeholder = if password.is_empty() { "" } else { "(unchanged)" };
    let classes = if visible { "card wifi-card" } else { "card wifi-card hidden" };

    format!(
        "<section class='{}' data-slot='{}'><div class='card-head'><h3>WiFi slot {}</h3><button class='danger remove-wifi-btn' type='button'>Remove</button></div><label>SSID<input list='wifi-options' name='wifi_ssid{}' value='{}' autocomplete='off'></label><label>Password<input type='password' name='wifi_password{}' value='' placeholder='{}' autocomplete='off'></label><label class='check'><input type='checkbox' name='wifi_no_password_{}' {}> No password</label></section>",
        classes,
        index,
        index,
        suffix,
        html_escape(ssid),
        suffix,
        placeholder,
        index,
        checked,
    )
}

async fn send_setup_page<T, const N: usize>(
    conn: &mut Connection<'_, T, N>,
    cfg: &AppConfig,
    notice: Option<&str>,
    close_after_save: bool,
) -> Result<(), Error<<T as ErrorType>::Error>>
where
    T: Read + Write,
{
    conn.initiate_response(
        200,
        None,
        &[
            ("Content-Type", "text/html; charset=utf-8"),
            ("Connection", "close"),
            ("Cache-Control", "no-store"),
        ],
    )
    .await?;

    conn.write_all(b"<!doctype html><html><head><meta charset='utf-8'><meta name='viewport' content='width=device-width,initial-scale=1'><title>Climate Monitor Setup</title><style>body{margin:0;font-family:Segoe UI,Arial,sans-serif;background:#ecf2f7;color:#17324a}main{max-width:860px;margin:0 auto;padding:18px}h1{margin:0 0 8px}.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(240px,1fr));gap:12px}.card{background:#fff;border:1px solid #c8d6e2;border-radius:12px;padding:14px}.hidden{display:none}.card-head{display:flex;justify-content:space-between;align-items:center}.actions{display:flex;gap:8px;flex-wrap:wrap}.toolbar{margin:10px 0 14px}.panel{margin-top:14px}.check{display:flex;gap:8px;align-items:center}.check input{width:auto;margin:0}label{display:block;font-size:14px;font-weight:600;margin-top:8px}input{width:100%;box-sizing:border-box;margin-top:5px;padding:9px;border:1px solid #b8c8d4;border-radius:8px}button{border:0;border-radius:999px;padding:10px 14px;font-weight:700;cursor:pointer}.primary{background:#0d6d5f;color:#fff}.secondary{background:#d7e5ee;color:#17324a}.danger{background:#f4d9d9;color:#7a1f1f;padding:7px 10px;font-size:12px}.notice{background:#e8fff8;border:1px solid #8fd0bc;padding:10px;border-radius:10px}pre{margin:10px 0 0;padding:10px;border-radius:8px;background:#0e2233;color:#d8ecf7;white-space:pre-wrap;max-height:220px;overflow:auto}small{color:#4e6478}</style></head><body><main><h1>Climate Monitor Setup</h1><p>Open only when needed.</p>").await?;

    if let Some(msg) = notice {
        conn.write_all(format!("<p class='notice'>{}</p>", html_escape(msg)).as_bytes())
            .await?;
    }

    conn.write_all(b"<div class='actions toolbar'><button class='secondary' type='button' id='scan-btn'>Scan WiFi</button><button class='secondary' type='button' id='add-wifi-btn'>Add new config</button><small id='scan-status'>Starting WiFi scan automatically...</small></div><form method='GET' action='/save'><div class='grid' id='wifi-grid'>").await?;

    let slots = [
        (1usize, cfg.wifi_ssid.as_str(), cfg.wifi_password.as_str()),
        (2usize, cfg.wifi_ssid_2.as_str(), cfg.wifi_password_2.as_str()),
        (3usize, cfg.wifi_ssid_3.as_str(), cfg.wifi_password_3.as_str()),
        (4usize, cfg.wifi_ssid_4.as_str(), cfg.wifi_password_4.as_str()),
        (5usize, cfg.wifi_ssid_5.as_str(), cfg.wifi_password_5.as_str()),
    ];
    let non_empty = slots.iter().filter(|(_, ssid, _)| !ssid.is_empty()).count();
    for (index, ssid, pass) in slots {
        let visible = !ssid.is_empty() || (non_empty == 0 && index == 1);
        conn.write_all(render_wifi_slot(index, ssid, pass, visible).as_bytes())
            .await?;
    }

    conn.write_all(b"</div><section class='card panel'><h3>Backend</h3><label>API URL<input name='api_url' value='").await?;
    conn.write_all(html_escape(&cfg.api_url).as_bytes()).await?;
    conn.write_all(b"'></label><label>API key<input name='api_key' value='").await?;
    conn.write_all(html_escape(&cfg.api_key).as_bytes()).await?;
    conn.write_all(b"'></label></section><section class='card panel'><h3>Calibration</h3><label>Temperature offset (C)<input name='temp_offset' value='").await?;
    conn.write_all(html_escape(&cfg.temp_offset).as_bytes()).await?;
    conn.write_all(b"'></label><label>Humidity offset (%)<input name='hum_offset' value='").await?;
    conn.write_all(html_escape(&cfg.hum_offset).as_bytes()).await?;
    conn.write_all(b"'></label></section><div class='actions panel'><button class='primary' type='submit'>Save settings</button></div></form><section class='card panel'><div class='actions'><h3 style='margin:0'>Runtime log</h3><button class='secondary' type='button' id='log-btn'>Refresh log</button></div><pre id='runtime-log'>Loading...</pre></section><datalist id='wifi-options'></datalist><script>const scanBtn=document.getElementById('scan-btn'),addWifiBtn=document.getElementById('add-wifi-btn'),scanStatus=document.getElementById('scan-status'),wifiOptions=document.getElementById('wifi-options'),wifiCards=[...document.querySelectorAll('.wifi-card')],logBtn=document.getElementById('log-btn'),runtimeLog=document.getElementById('runtime-log');function upd(){addWifiBtn.disabled=!wifiCards.find(c=>c.classList.contains('hidden'));}function one(){if(wifiCards.some(c=>!c.classList.contains('hidden')))return;const x=wifiCards.find(c=>c.classList.contains('hidden'));if(x)x.classList.remove('hidden');}wifiCards.forEach(card=>{const rm=card.querySelector('.remove-wifi-btn'),ssid=card.querySelector('input[name^=\"wifi_ssid\"]'),pass=card.querySelector('input[name^=\"wifi_password\"]'),np=card.querySelector('input[name^=\"wifi_no_password_\"]');rm.addEventListener('click',()=>{ssid.value='';pass.value='';np.checked=false;card.classList.add('hidden');one();upd();});});addWifiBtn.addEventListener('click',()=>{const n=wifiCards.find(c=>c.classList.contains('hidden'));if(!n)return;n.classList.remove('hidden');const i=n.querySelector('input[name^=\"wifi_ssid\"]');if(i)i.focus();upd();});upd();let retry=false;async function scan(a){scanBtn.disabled=true;scanStatus.textContent=a?'Retrying WiFi scan...':'Scanning...';try{const d=await fetch('/wifi-scan').then(r=>r.json());wifiOptions.innerHTML='';(d.networks||[]).forEach(n=>{const o=document.createElement('option');o.value=n.ssid;o.label=n.ssid+' ('+n.rssi+' dBm)';wifiOptions.appendChild(o);});if((d.networks||[]).length){scanStatus.textContent='Found '+d.networks.length+' networks.';}else if(!retry){retry=true;scanStatus.textContent='No networks yet, retrying in 3 s...';setTimeout(()=>scan(true),3000);}else{scanStatus.textContent='No networks found.';}}catch(_e){scanStatus.textContent='Scan failed.';}finally{scanBtn.disabled=false;}}async function refreshLog(){try{const t=await fetch('/wifi-log').then(r=>r.text());runtimeLog.textContent=t||'No log yet.';runtimeLog.scrollTop=runtimeLog.scrollHeight;}catch(_e){runtimeLog.textContent='Failed to load runtime log.';}}logBtn.addEventListener('click',refreshLog);scanBtn.addEventListener('click',()=>scan(false));scan(false);refreshLog();").await?;

    if close_after_save {
        conn.write_all(b"setTimeout(()=>{try{window.open('','_self');window.close();}catch(_e){}location.href='about:blank';},2000);")
            .await?;
    }

    conn.write_all(b"</script></main></body></html>").await?;
    conn.complete().await
}

async fn send_redirect<T, const N: usize>(
    conn: &mut Connection<'_, T, N>,
    location: &str,
) -> Result<(), Error<<T as ErrorType>::Error>>
where
    T: Read + Write,
{
    conn.initiate_response(
        302,
        None,
        &[
            ("Location", location),
            ("Cache-Control", "no-store"),
            ("Connection", "close"),
        ],
    )
    .await?;
    conn.complete().await
}

async fn send_text<T, const N: usize>(
    conn: &mut Connection<'_, T, N>,
    status: u16,
    content_type: &str,
    body: &str,
) -> Result<(), Error<<T as ErrorType>::Error>>
where
    T: Read + Write,
{
    conn.initiate_response(
        status,
        None,
        &[("Content-Type", content_type), ("Connection", "close")],
    )
    .await?;
    if !body.is_empty() {
        conn.write_all(body.as_bytes()).await?;
    }
    conn.complete().await
}

async fn send_text_string<T, const N: usize>(
    conn: &mut Connection<'_, T, N>,
    status: u16,
    content_type: &str,
    body: String,
) -> Result<(), Error<<T as ErrorType>::Error>>
where
    T: Read + Write,
{
    send_text(conn, status, content_type, &body).await
}