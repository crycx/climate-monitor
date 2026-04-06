//! Captive portal redirect: inspects the Host header and redirects non-AP requests.

use edge_http::io::Error;
use edge_http::io::server::Connection;
use embedded_io_async::{ErrorType, Read, Write};

use super::dhcp::{AP_HOST_NAME, AP_URL};
use super::responses::send_redirect;

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
    let host = headers
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("Host"))
        .map(|(_, value)| value);
    let redirect = host.map(|v| v != AP_HOST_NAME).unwrap_or(false);

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
