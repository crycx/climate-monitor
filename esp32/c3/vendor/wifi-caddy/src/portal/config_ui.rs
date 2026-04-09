//! Generic config UI handler: "/" (config page), "/config-group/:group", "/config/:field".
//!
//! Implements `edge_http::io::server::Handler` to route requests manually.
//! Use with a config type that implements `ConfigFormGen`, `ConfigApi`, and `ConfigLoadStore`;
//! storage is passed as a second mutex and must implement `ConfigStorage`.

extern crate alloc;

use core::fmt::{Debug, Display};

use crate::config_storage::{
    ConfigApi, ConfigChangedSet, ConfigFormGen, ConfigGet, ConfigLoadStore, ConfigStorage,
};
use edge_http::io::Error;
use edge_http::io::server::Connection;
use edge_nal::TcpSplit;
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::mutex::Mutex;
use embedded_io_async::{ErrorType, Read, Write};

use super::config_group::{ConfigGroupResult, ConfigQuery, handle_config_group};
use super::config_page::{ConfigPageChunks, EMPTY_SEGMENTS, PageTab, page_to_id};
use super::responses::{send_json, send_response_string, send_text, send_text_string};

/// Owned response returned by a custom GET route handler.
pub struct CustomResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response content type.
    pub content_type: &'static str,
    /// Response body.
    pub body: alloc::string::String,
}

/// Buffer size for JSON config-group responses.
const CONFIG_GROUP_JSON_BUF_SIZE: usize = 512;

/// HTTP request handler for the config UI.
///
/// Implements `edge_http::io::server::Handler` with manual routing for three endpoints:
/// - `GET /` -- config page (streamed HTML)
/// - `GET /config-group/<group>` -- config group JSON get/set via `?set=...`
/// - `GET /config/<field>` -- single field get/set via `?set=...`
pub struct ConfigHandler<R: RawMutex + 'static, C: ConfigApi + 'static, S: 'static> {
    /// Shared config mutex (read for GET, locked+mutated for SET).
    pub config: &'static Mutex<R, C>,
    /// Shared storage mutex (used to persist after SET).
    pub io: &'static Mutex<R, S>,
    /// Which tab/group to show first (e.g. `"main"`).
    pub default_group: &'static str,
    /// `<h1>` heading on the config page.
    pub page_heading: &'static str,
    /// `<title>` of the config page.
    pub title: &'static str,
    /// Subtitle shown below the heading.
    pub subtitle: &'static str,
    /// Left navigation HTML.
    pub nav_left: &'static str,
    /// Right navigation HTML.
    pub nav_right: &'static str,
    /// Extra CSS appended after the built-in stylesheet.
    pub extra_css: &'static str,
    /// Extra JavaScript appended after the built-in UI scripts.
    pub extra_js: &'static str,
    /// Callback invoked after a config update with the set of changed fields.
    pub on_updated: Option<&'static (dyn Fn(C::ChangedSet) + Send)>,
    /// Optional custom GET route handler for non-config endpoints.
    pub custom_get: Option<&'static (dyn Fn(&str) -> Option<CustomResponse> + Send + Sync)>,
    /// Whether to serve captive-portal redirects on this handler.
    #[cfg(feature = "captive")]
    pub captive: bool,
}

/// Extract the `set` query parameter value from a path like `/config-group/foo?set=...`.
fn parse_set_param(path: &str) -> Option<alloc::string::String> {
    let query = path.split_once('?')?.1;
    for pair in query.split('&') {
        if let Some(value) = pair.strip_prefix("set=") {
            return Some(percent_decode(value));
        }
    }
    None
}

/// Minimal percent-decoding for query values.
fn percent_decode(s: &str) -> alloc::string::String {
    let mut out = alloc::string::String::with_capacity(s.len());
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

/// Extract the path portion (before `?`) from the full request path.
fn path_only(full: &str) -> &str {
    full.split_once('?').map_or(full, |(p, _)| p)
}

impl<R, C, S> edge_http::io::server::Handler for ConfigHandler<R, C, S>
where
    R: RawMutex + 'static,
    C: ConfigFormGen + ConfigGet + ConfigApi + ConfigLoadStore + Send,
    S: ConfigStorage + Send,
{
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
        debug!(
            "http[{}]: {} {}",
            crate::fmt::DisplayFmt(&task_id),
            headers.method,
            full_path
        );

        #[cfg(feature = "captive")]
        if self.captive {
            if super::captive::check_captive_redirect(conn).await? {
                return Ok(());
            }
        }

        let path = path_only(full_path);

        match path {
            "/" => self.handle_root(conn).await,
            p if p.starts_with("/config-group/") => {
                let group = &p["/config-group/".len()..];
                self.handle_config_group(conn, group, full_path).await
            }
            p if p.starts_with("/config/") => {
                let field = &p["/config/".len()..];
                self.handle_config_field(conn, field, full_path).await
            }
            _ => {
                if let Some(handler) = self.custom_get {
                    if let Some(response) = handler(path) {
                        return send_response_string(
                            conn,
                            response.status,
                            response.content_type,
                            response.body,
                        )
                        .await;
                    }
                }
                send_text(conn, 404, "Not Found").await
            }
        }
    }
}

impl<R, C, S> ConfigHandler<R, C, S>
where
    R: RawMutex + 'static,
    C: ConfigFormGen + ConfigGet + ConfigApi + ConfigLoadStore + Send,
    S: ConfigStorage + Send,
{
    async fn handle_root<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), Error<<T as ErrorType>::Error>>
    where
        T: Read + Write,
    {
        let mut pages = alloc::vec::Vec::new();
        for name in C::page_names() {
            let html_segments = C::html_segments_for_group(name).unwrap_or(EMPTY_SEGMENTS);
            let js_segments = C::js_segments_for_group(name).unwrap_or(EMPTY_SEGMENTS);
            pages.push(PageTab {
                name,
                html_segments,
                js_segments,
            });
        }
        let default_page_id = page_to_id(self.default_group);
        let chunks = ConfigPageChunks {
            page_heading: self.page_heading,
            title: self.title,
            subtitle: self.subtitle,
            nav_left: self.nav_left,
            nav_right: self.nav_right,
            extra_css: self.extra_css,
            extra_js: self.extra_js,
            pages,
            default_page_id,
        };
        chunks.write_to(conn).await
    }

    async fn handle_config_group<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
        group: &str,
        full_path: &str,
    ) -> Result<(), Error<<T as ErrorType>::Error>>
    where
        T: Read + Write,
    {
        let query = ConfigQuery {
            set: parse_set_param(full_path),
        };
        let mut buf = [0u8; CONFIG_GROUP_JSON_BUF_SIZE];
        let result = handle_config_group(
            self.config,
            self.io,
            group,
            query,
            &mut buf,
            self.on_updated,
        )
        .await;
        match result {
            ConfigGroupResult::Json(json) => send_json(conn, &json).await,
            ConfigGroupResult::Err(status, msg) => send_text(conn, status, &msg).await,
        }
    }

    async fn handle_config_field<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
        field: &str,
        full_path: &str,
    ) -> Result<(), Error<<T as ErrorType>::Error>>
    where
        T: Read + Write,
    {
        if let Some(set_value) = parse_set_param(full_path) {
            let mut cfg = self.config.lock().await;
            match cfg.set_field(field, &set_value) {
                Ok(Some(changed)) => {
                    if !changed.is_empty() {
                        if let Err(_err) = cfg.store_to(&mut *self.io.lock().await).await {
                            error!("http: config store failed");
                            return send_text(conn, 500, "").await;
                        }
                        if let Some(f) = self.on_updated {
                            f(changed);
                        }
                    }
                    send_text_string(conn, 200, set_value).await
                }
                Ok(None) => send_text(conn, 400, "Invalid key or value").await,
                Err(_) => send_text(conn, 400, "Invalid key or value").await,
            }
        } else {
            match crate::config_storage::ConfigGet::get(&*self.config.lock().await, field) {
                Some(value) => send_text_string(conn, 200, value).await,
                None => send_text(conn, 404, "").await,
            }
        }
    }
}
