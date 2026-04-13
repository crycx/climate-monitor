use embassy_net::{
    dns::DnsSocket,
    tcp::client::{TcpClient, TcpClientState},
    Stack,
};
use embassy_time::Duration;
use reqwless::{
    headers::ContentType,
    request::Method,
};
#[cfg(not(feature = "no-tls"))]
use reqwless::client::{HttpClient, TlsConfig, TlsVerify};
#[cfg(feature = "no-tls")]
use reqwless::client::HttpClient;

use crate::server_time_response::extract_server_unix_from_headers;
use crate::{HTTP_CONNECT_TIMEOUT_SECS, HTTP_SEND_TIMEOUT_SECS};

type PostResult = (bool, Option<u64>, Option<alloc::string::String>);

fn fail(msg: alloc::string::String) -> PostResult {
    crate::logln!("{}", msg);
    (false, None, Some(msg))
}

pub async fn post_readings(
    stack: &'static Stack<'static>,
    tcp_state: &'static TcpClientState<1, 4096, 4096>,
    api_url: &str,
    api_key: &str,
    body: &[u8],
    wifi_connected: Option<bool>,
) -> PostResult {
    if api_url.is_empty() || api_key.is_empty() {
        return fail(alloc::format!("API config missing, skipping POST"));
    }
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

    let readings_url = alloc::format!("{}readings", api_url);
    let mut rx_buf = [0u8; 512];

    let request = match embassy_time::with_timeout(
        Duration::from_secs(HTTP_CONNECT_TIMEOUT_SECS),
        http.request(Method::POST, readings_url.as_str()),
    )
    .await
    {
        Err(_) => {
            let _ = crate::network::verify_dns(stack, api_url, wifi_connected).await;
            return fail(alloc::format!(
                "HTTP connect timeout after {}s",
                HTTP_CONNECT_TIMEOUT_SECS
            ));
        }
        Ok(Err(error)) => {
            return fail(alloc::format!("HTTP connect error: {:?}", error));
        }
        Ok(Ok(request)) => request,
    };

    let headers = [("X-API-Key", api_key)];
    let mut builder = request
        .content_type(ContentType::ApplicationJson)
        .headers(&headers)
        .body(body);

    let response = match embassy_time::with_timeout(
        Duration::from_secs(HTTP_SEND_TIMEOUT_SECS),
        builder.send(&mut rx_buf),
    )
    .await
    {
        Err(_) => {
            return fail(alloc::format!(
                "POST send timeout after {}s",
                HTTP_SEND_TIMEOUT_SECS
            ));
        }
        Ok(response) => response,
    };

    match response {
        Ok(response) => {
            let sec = extract_server_unix_from_headers(response.headers());
            crate::logln!("POST -> HTTP {}", response.status.0);
            (true, sec, None)
        }
        Err(error) => fail(alloc::format!("POST error: {:?}", error)),
    }
}
