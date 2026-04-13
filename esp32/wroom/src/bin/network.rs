use core::sync::atomic::{AtomicBool, Ordering};
use embassy_net::{dns::DnsQueryType, tcp::TcpSocket, IpEndpoint, Stack};
use embassy_time::Duration;

#[embassy_executor::task]
pub async fn net_task(mut runner: embassy_net::Runner<'static, esp_radio::wifi::WifiDevice<'static>>) {
    runner.run().await;
}

static DNS_ERROR_DETAIL_LOGGED: AtomicBool = AtomicBool::new(false);
static DNS_FAILURE_STREAK: AtomicBool = AtomicBool::new(false);

pub async fn verify_dns(
    stack: &'static Stack<'static>,
    api_url: &str,
    wifi_connected: Option<bool>,
) -> bool {
    let hostname = {
        let after_scheme = api_url
            .find("://")
            .map(|i| &api_url[i + 3..])
            .unwrap_or(api_url);
        after_scheme
            .find('/')
            .map(|i| &after_scheme[..i])
            .unwrap_or(after_scheme)
    };

    if !DNS_FAILURE_STREAK.load(Ordering::Relaxed) {
        log_net_status(stack, wifi_connected);
        probe_gateway(stack).await;
        crate::logln!("Resolving {} ...", hostname);
    }

    match stack.dns_query(hostname, DnsQueryType::A).await {
        Ok(addresses) => {
            if DNS_FAILURE_STREAK.swap(false, Ordering::Relaxed) {
                crate::logln!("DNS recovered");
            }
            crate::logln!("DNS resolved: {:?}", addresses);
            true
        }
        Err(error) => {
            if let Some(connected) = wifi_connected {
                crate::logln!("Wi-Fi connected: {}", connected);
            }
            log_dns_error_details_once(error);
            crate::logln!("DNS query failed: {:?}", error);
            DNS_FAILURE_STREAK.store(true, Ordering::Relaxed);
            false
        }
    }
}

fn log_net_status(stack: &'static Stack<'static>, wifi_connected: Option<bool>) {
    crate::logln!(
        "Net status: link_up={}, config_up={}",
        stack.is_link_up(),
        stack.is_config_up()
    );
    if let Some(connected) = wifi_connected {
        crate::logln!("Wi-Fi connected: {}", connected);
    }
    if let Some(cfg) = stack.config_v4() {
        crate::logln!("Gateway: {:?}", cfg.gateway);
        crate::logln!("DNS servers: {:?}", cfg.dns_servers);
        crate::logln!("IP address: {:?}", cfg.address);
    } else {
        crate::logln!("No IPv4 config yet");
    }
}

async fn probe_gateway(stack: &'static Stack<'static>) {
    let Some(cfg) = stack.config_v4() else {
        crate::logln!("Gateway probe skipped: no IPv4 config");
        return;
    };
    let Some(gateway) = cfg.gateway else {
        crate::logln!("Gateway probe skipped: no gateway");
        return;
    };

    let mut rx_buf = [0u8; 64];
    let mut tx_buf = [0u8; 64];
    let mut socket = TcpSocket::new(*stack, &mut rx_buf, &mut tx_buf);

    match embassy_time::with_timeout(
        Duration::from_secs(2),
        socket.connect(IpEndpoint::new(embassy_net::IpAddress::Ipv4(gateway), 53)),
    )
    .await
    {
        Ok(Ok(())) => crate::logln!("Gateway reachability probe: TCP connect ok"),
        Ok(Err(error)) => {
            crate::logln!("Gateway reachability probe: TCP connect failed: {:?}", error)
        }
        Err(_) => crate::logln!("Gateway reachability probe: TCP connect timeout"),
    }
}

fn log_dns_error_details_once(error: embassy_net::dns::Error) {
    if !DNS_ERROR_DETAIL_LOGGED.swap(true, Ordering::Relaxed) {
        crate::logln!(
            "DNS error detail (once): {:?}; raw DNS code not exposed by embassy-net",
            error
        );
    }
}
