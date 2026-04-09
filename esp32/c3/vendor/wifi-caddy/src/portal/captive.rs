//! Captive portal redirect: inspects the Host header and redirects non-AP requests.

use edge_http::io::Error;
use edge_http::io::server::Connection;
use embedded_io_async::{ErrorType, Read, Write};

use super::dhcp::{AP_HOST_NAME, AP_URL};
use super::responses::send_redirect;

fn host_without_port(host: &str) -> &str {
    host.split(':').next().unwrap_or(host)
}

fn is_probe_path(path: &str) -> bool {
    matches!(
        path,
        "/connecttest.txt"
            | "/redirect"
            | "/hotspot-detect.html"
            | "/generate_204"
            | "/gen_204"
            | "/ncsi.txt"
            | "/success.txt"
    )
}

/// Check if the request should be redirected to the AP URL (captive portal).
/// Returns `true` if a redirect response was sent (caller should not process further).
/// Returns `false` if the request should be handled normally.
pub async fn check_captive_redirect<T, const N: usize>(
    conn: &mut Connection<'_, T, N>,
) -> Result<bool, Error<<T as ErrorType>::Error>>
where
    T: Read + Write,
{
    let headers = conn.headers()?;
    let path = headers.path.split_once('?').map_or(headers.path, |(p, _)| p);
    if is_probe_path(path) {
        info!("captive: probe path '{}' -> redirect to portal", path);
        send_redirect(conn, AP_URL).await?;
        return Ok(true);
    }

    let host = headers
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("Host"))
        .map(|(_, value)| value);
    let redirect = host
        .map(|value| host_without_port(value) != AP_HOST_NAME)
        .unwrap_or(true);

    if redirect {
        info!(
            "captive: redirecting to {} (host: {:?})",
            AP_URL,
            crate::fmt::DebugFmt(&host)
        );
        send_redirect(conn, AP_URL).await?;
        Ok(true)
    } else {
        debug!(
            "captive: host={:?}, serving normally",
            crate::fmt::DebugFmt(&host)
        );
        Ok(false)
    }
}
