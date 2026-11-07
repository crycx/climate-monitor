//! WiFi setup portal: open AP `ClimateSensor-xxxxxx`, form at
//! http://192.168.4.1/. Started by pressing BOOT during the boot window.
//! Saving restarts the sensor; 5 minutes without a request also restarts it,
//! so a forgotten portal cannot drain the battery.

use alloc::{borrow::ToOwned, format, string::String, vec::Vec};
use core::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use edge_dhcp::io::DEFAULT_SERVER_PORT;
use edge_dhcp::server::{Server as DhcpServer, ServerOptions};
use edge_nal::UdpBind;
use edge_nal_embassy::{Udp, UdpBuffers};
use embassy_executor::Spawner;
use embassy_net::{Ipv4Address, Ipv4Cidr, Runner, Stack, StackResources, StaticConfigV4, tcp::TcpSocket};
use embassy_time::{Duration, Timer, with_timeout};
use esp_hal::{gpio::Output, peripherals::WIFI, rng::Rng};
use esp_radio::wifi::{Config as ModeConfig, ControllerConfig, Interface, ap::AccessPointConfig};

use crate::config::{ConfigStore, NodeConfig, MAX_INTERVAL_SECS, MIN_INTERVAL_SECS};
use crate::form::{html_escape, parse_form};
use crate::hw::{self, Measurement, Sensors};
use crate::log_line;

const AP_IP: Ipv4Addr = Ipv4Addr::new(192, 168, 4, 1);
const AP_CHANNEL: u8 = 6;
const IDLE_TIMEOUT_SECS: u64 = 300;
const MAX_HEADER: usize = 4096;
const MAX_BODY: usize = 2048;

macro_rules! mk_static {
    ($t:ty, $val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        STATIC_CELL.uninit().write($val)
    }};
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, Interface<'static>>) {
    runner.run().await;
}

#[embassy_executor::task]
async fn dhcp_task(stack: Stack<'static>) {
    loop {
        stack.wait_link_up().await;
        let buffers = UdpBuffers::<1, 1500, 1500, 2>::new();
        let udp = Udp::new(stack, &buffers);
        let Ok(mut socket) = udp
            .bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DEFAULT_SERVER_PORT)))
            .await
        else {
            Timer::after(Duration::from_secs(1)).await;
            continue;
        };
        let mut server: DhcpServer<_, 16> = DhcpServer::new_with_et(AP_IP);
        server.range_start = Ipv4Addr::new(192, 168, 4, 100);
        server.range_end = Ipv4Addr::new(192, 168, 4, 150);
        let mut gw_buf = [Ipv4Addr::UNSPECIFIED];
        let options = ServerOptions::new(AP_IP, Some(&mut gw_buf));
        let mut buf = [0u8; 1500];
        let _ = edge_dhcp::io::server::run(&mut server, &options, &mut socket, &mut buf).await;
        Timer::after(Duration::from_secs(1)).await;
    }
}

struct Request {
    method: String,
    path: String,
    body: Vec<u8>,
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

async fn read_request(socket: &mut TcpSocket<'_>) -> Option<Request> {
    let mut data: Vec<u8> = Vec::new();
    let mut buf = [0u8; 512];
    let header_end = loop {
        let n = socket.read(&mut buf).await.ok()?;
        if n == 0 {
            return None;
        }
        data.extend_from_slice(&buf[..n]);
        if let Some(pos) = find(&data, b"\r\n\r\n") {
            break pos + 4;
        }
        if data.len() > MAX_HEADER {
            return None;
        }
    };
    let head = core::str::from_utf8(&data[..header_end]).ok()?;
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split(' ');
    let method = request_line.next()?.to_owned();
    let path = request_line.next()?.to_owned();
    let content_length = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
        .unwrap_or(0)
        .min(MAX_BODY);
    while data.len() < header_end + content_length {
        let n = socket.read(&mut buf).await.ok()?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
    }
    let body_end = data.len().min(header_end + content_length);
    Some(Request {
        method,
        path,
        body: data[header_end..body_end].to_vec(),
    })
}

fn page(cfg: &NodeConfig, m: &Measurement, mac: &str, notice: &str, errors: &[String]) -> String {
    let reading = match m.climate {
        Some((t, h)) => format!("{:.1} &deg;C, {:.1} %", t, h),
        None => String::from("DHT22 not responding"),
    };
    let battery = if m.battery_mv > 0 {
        format!("{:.2} V ({} %)", m.battery_mv as f32 / 1000.0, crate::espnow_packet::lipo_percent(m.battery_mv))
    } else {
        String::from("unknown")
    };
    let mut messages = String::new();
    if !notice.is_empty() {
        messages.push_str(&format!("<p class='ok'>{}</p>", html_escape(notice)));
    }
    for e in errors {
        messages.push_str(&format!("<p class='err'>{}</p>", html_escape(e)));
    }
    let checked = |b: bool| if b { " checked" } else { "" };
    let selected = |b: bool| if b { " selected" } else { "" };
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Climate Sensor</title><style>
body{{font-family:system-ui,sans-serif;margin:0;padding:16px;background:#f4f5f7;color:#1d2330}}
main{{max-width:420px;margin:auto}}h1{{font-size:1.3em;margin:0 0 4px}}
.card{{background:#fff;border-radius:10px;padding:14px 16px;margin:12px 0;box-shadow:0 1px 3px #0002}}
label{{display:block;margin:10px 0 2px;font-size:.9em;color:#4a5263}}
input,select{{width:100%;box-sizing:border-box;padding:9px;font-size:1em;border:1px solid #c8ccd4;border-radius:6px}}
.row{{display:flex;gap:10px}}.row>div{{flex:1}}.check{{display:flex;align-items:center;gap:8px}}.check input{{width:auto}}
button{{width:100%;padding:12px;margin-top:16px;font-size:1em;border:0;border-radius:8px;background:#2563eb;color:#fff}}
small{{color:#6b7280}}.ok{{color:#15803d}}.err{{color:#b91c1c}}dl{{margin:0}}dt{{float:left;width:7em;color:#6b7280}}dd{{margin:0 0 4px 7em}}
</style></head><body><main>
<h1>Climate Sensor</h1><small>MAC {mac}</small>{messages}
<div class="card"><dl><dt>Reading</dt><dd>{reading}</dd><dt>Battery</dt><dd>{battery}</dd></dl></div>
<form method="post" action="/save" class="card">
<label>Name (Home Assistant)</label><input name="name" maxlength="24" value="{name}">
<label>Interval (seconds)</label><input name="interval" type="number" min="{min_iv}" max="{max_iv}" value="{interval}">
<small>300 = 5 min, 900 = 15 min</small>
<div class="row"><div><label>Temp offset (&deg;C)</label><input name="temp_offset" type="number" step="0.1" value="{temp_offset:.1}"></div>
<div><label>Humidity offset (%)</label><input name="hum_offset" type="number" step="0.1" value="{hum_offset:.1}"></div></div>
<label>ESP-NOW key</label><input name="key" type="password" autocomplete="off" placeholder="{key_placeholder}">
<small>Must match the gateway. Leave empty to keep the current key.</small>
<label>Antenna</label><select name="antenna"><option value="internal"{ant_int}>Internal (ceramic)</option><option value="external"{ant_ext}>External (U.FL)</option></select>
<div class="row"><div><label>DHT22 warm-up (ms)</label><input name="warmup" type="number" min="0" max="5000" value="{warmup}"></div>
<div><label>Divider ratio</label><input name="divider" type="number" step="0.01" min="1" max="10" value="{divider:.2}"></div></div>
<label class="check"><input type="checkbox" name="battery" value="on"{battery_checked}> Measure battery on D0</label>
<button type="submit">Save and restart</button></form>
<small>The portal closes after 5 minutes without activity.</small></main></body></html>"#,
        mac = mac,
        messages = messages,
        reading = reading,
        battery = battery,
        name = html_escape(&cfg.name),
        min_iv = MIN_INTERVAL_SECS,
        max_iv = MAX_INTERVAL_SECS,
        interval = cfg.interval_secs,
        temp_offset = cfg.temp_offset,
        hum_offset = cfg.hum_offset,
        key_placeholder = if cfg.has_key() { "(set)" } else { "not set - required" },
        ant_int = selected(!cfg.external_antenna),
        ant_ext = selected(cfg.external_antenna),
        warmup = cfg.dht_warmup_ms,
        divider = cfg.battery_divider,
        battery_checked = checked(cfg.battery_adc),
    )
}

async fn write_all(socket: &mut TcpSocket<'_>, mut data: &[u8]) -> bool {
    while !data.is_empty() {
        match socket.write(data).await {
            Ok(0) | Err(_) => return false,
            Ok(n) => data = &data[n..],
        }
    }
    true
}

async fn respond(socket: &mut TcpSocket<'_>, status: &str, extra_headers: &str, body: &str) {
    let head = format!(
        "HTTP/1.1 {}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n{}\r\n",
        status,
        body.len(),
        extra_headers
    );
    if write_all(socket, head.as_bytes()).await {
        let _ = write_all(socket, body.as_bytes()).await;
    }
    let _ = socket.flush().await;
}

/// Applies a submitted form. Returns the errors (empty = valid).
fn apply_form(cfg: &mut NodeConfig, form: &[(String, String)]) -> Vec<String> {
    let mut errors = Vec::new();
    for (field, value) in form {
        match field.as_str() {
            "key" if value.trim().is_empty() => {}
            "name" | "interval" | "temp_offset" | "hum_offset" | "key" | "antenna" | "warmup" | "divider" => {
                if let Err(e) = cfg.set(field, value) {
                    errors.push(e);
                }
            }
            _ => {}
        }
    }
    // Unchecked checkboxes are simply absent from the form.
    cfg.battery_adc = form.iter().any(|(k, _)| k == "battery");
    errors
}

pub async fn run(
    spawner: Spawner,
    mut cfg: NodeConfig,
    mut store: ConfigStore,
    mut sensors: Sensors,
    wifi: WIFI<'static>,
    rf: (Output<'static>, Output<'static>),
    _led: Output<'static>,
) -> ! {
    let measurement = sensors.measure(&cfg).await;

    let (mut controller, interfaces) =
        esp_radio::wifi::new(wifi, ControllerConfig::default()).expect("WiFi init failed");
    let _rf = rf;
    // HA identifies the sensor by its station MAC (what ESP-NOW sends from).
    let mac = hw::mac_str(&interfaces.station.mac_address());
    let ap_mac = interfaces.access_point.mac_address();
    let ssid = format!("ClimateSensor-{:02x}{:02x}{:02x}", ap_mac[3], ap_mac[4], ap_mac[5]);
    controller
        .set_config(&ModeConfig::AccessPoint(
            AccessPointConfig::default().with_ssid(ssid.clone()).with_channel(AP_CHANNEL),
        ))
        .expect("AP start failed");

    let rng = Rng::new();
    let seed = (rng.random() as u64) << 32 | rng.random() as u64;
    let net_config = embassy_net::Config::ipv4_static(StaticConfigV4 {
        address: Ipv4Cidr::new(Ipv4Address::new(192, 168, 4, 1), 24),
        gateway: Some(Ipv4Address::new(192, 168, 4, 1)),
        dns_servers: Default::default(),
    });
    let (stack, runner) = embassy_net::new(
        interfaces.access_point,
        net_config,
        mk_static!(StackResources<4>, StackResources::<4>::new()),
        seed,
    );
    spawner.spawn(net_task(runner).unwrap());
    spawner.spawn(dhcp_task(stack).unwrap());
    log_line!("Portal: WiFi '{}' open, http://192.168.4.1/", ssid);

    let mut rx_buf = [0u8; 1536];
    let mut tx_buf = [0u8; 2048];
    loop {
        let mut socket = TcpSocket::new(stack, &mut rx_buf, &mut tx_buf);
        socket.set_timeout(Some(Duration::from_secs(10)));
        match with_timeout(Duration::from_secs(IDLE_TIMEOUT_SECS), socket.accept(80)).await {
            Err(_) => {
                log_line!("Portal: idle timeout, restarting");
                esp_hal::system::software_reset();
            }
            Ok(Err(_)) => continue,
            Ok(Ok(())) => {}
        }

        let Some(request) = read_request(&mut socket).await else {
            socket.abort();
            continue;
        };
        let mut restart = false;
        match (request.method.as_str(), request.path.as_str()) {
            ("GET", "/") => {
                let body = page(&cfg, &measurement, &mac, "", &[]);
                respond(&mut socket, "200 OK", "", &body).await;
            }
            ("POST", "/save") => {
                let form = parse_form(&request.body);
                let mut candidate = cfg.clone();
                let mut errors = apply_form(&mut candidate, &form);
                if errors.is_empty() {
                    match store.save(&candidate) {
                        Ok(()) => {
                            cfg = candidate.clone();
                            restart = true;
                        }
                        Err(e) => errors.push(e.into()),
                    }
                }
                let body = if restart {
                    log_line!("Portal: settings saved, restarting");
                    page(&cfg, &measurement, &mac, "Saved. The sensor restarts and continues on battery.", &[])
                } else {
                    page(&candidate, &measurement, &mac, "", &errors)
                };
                respond(&mut socket, "200 OK", "", &body).await;
            }
            // Phones probe random URLs for captive portals: send them to the form.
            _ => respond(&mut socket, "302 Found", "Location: http://192.168.4.1/\r\n", "").await,
        }
        socket.close();
        Timer::after(Duration::from_millis(300)).await;
        if restart {
            Timer::after(Duration::from_millis(700)).await;
            esp_hal::system::software_reset();
        }
    }
}
