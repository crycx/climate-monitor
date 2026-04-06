//! HTTP response helpers for edge-http.

use alloc::string::String;

use edge_http::io::Error;
use edge_http::io::server::Connection;
use embedded_io_async::{ErrorType, Read, Write};

/// Send a 200 OK response with JSON content-type.
pub async fn send_json<T, const N: usize>(
    conn: &mut Connection<'_, T, N>,
    json: &str,
) -> Result<(), Error<<T as ErrorType>::Error>>
where
    T: Read + Write,
{
    conn.initiate_response(
        200,
        None,
        &[
            ("Content-Type", "application/json"),
            ("Connection", "close"),
        ],
    )
    .await?;
    conn.write_all(json.as_bytes()).await?;
    conn.complete().await
}

/// Send a response with plain-text content-type.
pub async fn send_text<T, const N: usize>(
    conn: &mut Connection<'_, T, N>,
    status: u16,
    body: &str,
) -> Result<(), Error<<T as ErrorType>::Error>>
where
    T: Read + Write,
{
    conn.initiate_response(
        status,
        None,
        &[("Content-Type", "text/plain"), ("Connection", "close")],
    )
    .await?;
    if !body.is_empty() {
        conn.write_all(body.as_bytes()).await?;
    }
    conn.complete().await
}

/// Send a response with plain-text content-type and an owned String body.
pub async fn send_text_string<T, const N: usize>(
    conn: &mut Connection<'_, T, N>,
    status: u16,
    body: String,
) -> Result<(), Error<<T as ErrorType>::Error>>
where
    T: Read + Write,
{
    send_text(conn, status, &body).await
}

/// Send a response with a caller-provided content-type and an owned String body.
pub async fn send_response_string<T, const N: usize>(
    conn: &mut Connection<'_, T, N>,
    status: u16,
    content_type: &str,
    body: String,
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

/// Send a 307 redirect response.
#[cfg(feature = "captive")]
pub async fn send_redirect<T, const N: usize>(
    conn: &mut Connection<'_, T, N>,
    location: &str,
) -> Result<(), Error<<T as ErrorType>::Error>>
where
    T: Read + Write,
{
    conn.initiate_response(
        307,
        None,
        &[("Location", location), ("Connection", "close")],
    )
    .await?;
    conn.write_all(location.as_bytes()).await?;
    conn.complete().await
}
