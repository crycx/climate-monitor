//! Generic HTTP config server loops (platform-agnostic).

use crate::config_storage::{ConfigApi, ConfigFormGen, ConfigGet, ConfigLoadStore, ConfigStorage};
use crate::portal;
use crate::portal::config_ui::{ConfigHandler, CustomResponse};
use embassy_net::Stack;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;

/// UI strings for the config page (provided by the proc-macro-generated `__ui_options()` method).
#[doc(hidden)]
#[derive(Clone, Copy, Debug)]
pub struct ConfigUiOptions {
    /// Default config group (e.g. `"basic"`).
    pub default_group: &'static str,
    /// Heading on the config page (e.g. `"WiFi Blink"`).
    pub page_heading: &'static str,
    /// Page title (e.g. `"WiFi Blink - Configuration"`).
    pub title: &'static str,
    /// Subtitle (e.g. `"WiFi and LED settings"`).
    pub subtitle: &'static str,
    /// Left nav HTML (e.g. `"<span>Configuration</span>"`).
    pub nav_left: &'static str,
    /// Right nav HTML (e.g. `"<span></span>"`).
    pub nav_right: &'static str,
    /// Extra CSS appended after the built-in stylesheet (e.g. overrides for colors or layout).
    pub extra_css: &'static str,
    /// Extra JavaScript appended after the built-in UI scripts.
    pub extra_js: &'static str,
}

/// Build a `ConfigHandler` from the shared config/io mutexes and UI options,
/// then run the HTTP server on the given stack.
///
/// Generic over the storage backend `S: ConfigStorage + Send`.
/// edge-http `Server::run` manages concurrent handlers internally, so this
/// replaces the old multi-task worker pool with a single embassy task.
#[doc(hidden)]
pub async fn run_http_config_loop<C, S>(
    stack: Stack<'static>,
    config: &'static Mutex<CriticalSectionRawMutex, C>,
    io: &'static Mutex<CriticalSectionRawMutex, S>,
    ui: ConfigUiOptions,
    on_updated: Option<&'static (dyn Fn(C::ChangedSet) + Send)>,
    custom_get: Option<&'static (dyn Fn(&str) -> Option<CustomResponse> + Send + Sync)>,
) where
    C: ConfigFormGen + ConfigGet + ConfigApi + ConfigLoadStore + Send,
    C::ChangedSet: Send,
    S: ConfigStorage + Send,
{
    info!("Starting HTTP config server...");

    loop {
        let handler = ConfigHandler {
            config,
            io,
            default_group: ui.default_group,
            page_heading: ui.page_heading,
            title: ui.title,
            subtitle: ui.subtitle,
            nav_left: ui.nav_left,
            nav_right: ui.nav_right,
            extra_css: ui.extra_css,
            extra_js: ui.extra_js,
            on_updated,
            custom_get,
            #[cfg(feature = "captive")]
            captive: true,
        };

        portal::serve_loop(stack, handler).await;

        // AP link went down (APDown command). Wait briefly before the next
        // wait_link_up() so we don't spin instantly.
        embassy_time::Timer::after(embassy_time::Duration::from_secs(2)).await;
    }
}

/// Debug HTTP config loop: runs on the STA interface without DHCP, DNS, or captive portal.
///
/// Waits for the STA stack to obtain an IPv4 address, then enters `serve_loop_debug`.
#[cfg(feature = "debug-server")]
#[doc(hidden)]
pub async fn run_http_debug_loop<C, S>(
    stack: Stack<'static>,
    config: &'static Mutex<CriticalSectionRawMutex, C>,
    io: &'static Mutex<CriticalSectionRawMutex, S>,
    ui: ConfigUiOptions,
    on_updated: Option<&'static (dyn Fn(C::ChangedSet) + Send)>,
    custom_get: Option<&'static (dyn Fn(&str) -> Option<CustomResponse> + Send + Sync)>,
) where
    C: ConfigFormGen + ConfigGet + ConfigApi + ConfigLoadStore + Send,
    C::ChangedSet: Send,
    S: ConfigStorage + Send,
{
    info!("Debug HTTP server: waiting for STA IP...");
    loop {
        if let Some(cfg) = stack.config_v4() {
            info!(
                "Debug HTTP server started at http://{}",
                cfg.address.address()
            );
            break;
        }
        embassy_time::Timer::after(embassy_time::Duration::from_millis(500)).await;
    }

    let handler = ConfigHandler {
        config,
        io,
        default_group: ui.default_group,
        page_heading: ui.page_heading,
        title: ui.title,
        subtitle: ui.subtitle,
        nav_left: ui.nav_left,
        nav_right: ui.nav_right,
        extra_css: ui.extra_css,
        extra_js: ui.extra_js,
        on_updated,
        custom_get,
        #[cfg(feature = "captive")]
        captive: false,
    };

    portal::serve_loop_debug(stack, handler).await;
}
