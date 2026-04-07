use embassy_net::{dns::DnsQueryType, Stack};

use crate::READINGS_URL;

#[embassy_executor::task]
pub async fn net_task(mut runner: embassy_net::Runner<'static, esp_radio::wifi::WifiDevice<'static>>) {
    runner.run().await;
}

pub async fn verify_dns(stack: &'static Stack<'static>) -> bool {
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

    crate::logln!("Resolving {} ...", hostname);
    match stack.dns_query(hostname, DnsQueryType::A).await {
        Ok(addresses) => {
            crate::logln!("DNS resolved: {:?}", addresses);
            true
        }
        Err(error) => {
            crate::logln!("DNS query failed: {:?}", error);
            false
        }
    }
}
