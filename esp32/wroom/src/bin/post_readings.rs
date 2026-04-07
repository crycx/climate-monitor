use embassy_net::{
    dns::DnsSocket,
    tcp::client::{TcpClient, TcpClientState},
    Stack,
};
use embassy_time::Duration;
use reqwless::{
    client::{HttpClient, TlsConfig, TlsVerify},
    headers::ContentType,
    request::{Method, RequestBuilder},
};

use crate::server_time_response::extract_server_unix_from_headers;
use crate::{API_KEY, HTTP_CONNECT_TIMEOUT_SECS, HTTP_SEND_TIMEOUT_SECS, READINGS_URL};

pub async fn post_readings(
    stack: &'static Stack<'static>,
    tcp_state: &'static TcpClientState<1, 4096, 4096>,
    temp: f32,
    hum: f32,
) -> (bool, Option<u64>, Option<alloc::string::String>) {
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
    let mut rx_buf = [0u8; 512];

    match embassy_time::with_timeout(
        Duration::from_secs(HTTP_CONNECT_TIMEOUT_SECS),
        http.request(Method::POST, READINGS_URL),
    )
    .await
    {
        Err(_) => {
            let msg = alloc::format!("HTTP connect timeout after {}s", HTTP_CONNECT_TIMEOUT_SECS);
            crate::logln!("{}", msg);
            (false, None, Some(msg))
        }
        Ok(request) => {
            let request = match request {
                Ok(request) => request,
                Err(error) => {
                    let msg = alloc::format!("HTTP connect error: {:?}", error);
                    crate::logln!("{}", msg);
                    return (false, None, Some(msg));
                }
            };

            let headers = [("X-API-Key", API_KEY)];
            let mut builder = request
                .content_type(ContentType::ApplicationJson)
                .headers(&headers)
                .body(body.as_bytes());
            match embassy_time::with_timeout(
                Duration::from_secs(HTTP_SEND_TIMEOUT_SECS),
                builder.send(&mut rx_buf),
            )
            .await
            {
                Err(_) => {
                    let msg = alloc::format!("POST send timeout after {}s", HTTP_SEND_TIMEOUT_SECS);
                    crate::logln!("{}", msg);
                    (false, None, Some(msg))
                }
                Ok(response) => match response {
                    Ok(response) => {
                        let sec = extract_server_unix_from_headers(response.headers());
                        crate::logln!("POST -> HTTP {}", response.status.0);
                        (true, sec, None)
                    }
                    Err(error) => {
                        let msg = alloc::format!("POST error: {:?}", error);
                        crate::logln!("{}", msg);
                        (false, None, Some(msg))
                    }
                },
            }
        }
    }
}
