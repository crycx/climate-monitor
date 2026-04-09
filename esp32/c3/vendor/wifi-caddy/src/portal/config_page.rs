//! Build a full HTML config form page from a heading, title, subtitle, nav, and form body/JS.
//!
//! The app supplies all content strings (form body and JS typically from `ConfigFormGen`).
//! Streams the HTML page directly to an edge-http `Connection` (chunked transfer encoding
//! is used automatically when no Content-Length header is set).

use alloc::string::String;

use edge_http::io::Error;
use edge_http::io::server::Connection;
use embedded_io_async::{ErrorType, Read, Write};

const CONFIG_PAGE_CSS: &str = include_str!("config_page.css");

/// Converts page name to a valid JS/HTML id suffix. Must match wifi-caddy-proc's page_name_to_js_id.
pub fn page_to_id(page: &str) -> String {
    page.replace('-', "_").replace(' ', "_")
}

const CONFIG_PAGE_TAB_SCRIPT: &str = r#"
var messageEl = document.getElementById('message');
function showMessage(text, isError) {
    if (!messageEl) return;
    messageEl.textContent = text;
    messageEl.className = 'message ' + (isError ? 'error' : 'success') + ' show';
    setTimeout(function() { messageEl.classList.remove('show'); }, 5000);
}
var loaded = {};
function switchTab(page) {
    document.querySelectorAll('.config-tab').forEach(function(b) { b.classList.remove('active'); });
    document.querySelectorAll('.config-tab-panel').forEach(function(p) { p.style.display = 'none'; });
    var btn = document.querySelector('.config-tab[data-page="' + page + '"]');
    if (btn) btn.classList.add('active');
    var panel = document.getElementById('panel-' + page);
    if (panel) panel.style.display = '';
    if (!loaded[page]) {
        loaded[page] = true;
        var loadFn = window['loadConfig_' + page];
        if (loadFn) {
            loadFn().then(function() {
                panel.classList.add('loaded');
                panel.querySelectorAll('input,button').forEach(function(el) { el.disabled = false; });
                showMessage('Configuration loaded');
            }).catch(function(err) {
                var ov = document.getElementById('loading-' + page);
                if (ov) ov.textContent = 'Load failed: ' + err.message;
                var rb = panel.querySelector('.reloadBtn');
                if (rb) rb.disabled = false;
            });
        }
    }
}
document.querySelectorAll('.config-tab').forEach(function(btn) {
    btn.addEventListener('click', function() { switchTab(this.dataset.page); });
});
document.querySelectorAll('.config-tab-panel').forEach(function(panel) {
    panel.querySelectorAll('input,button').forEach(function(el) { el.disabled = true; });
});
document.querySelectorAll('.config-tab-panel form').forEach(function(form) {
    form.addEventListener('submit', function(e) {
        e.preventDefault();
        var page = this.id.replace('configForm-', '');
        var saveFn = window['saveConfig_' + page];
        if (saveFn) saveFn().then(function() { showMessage('Configuration saved'); })
            .catch(function(err) { showMessage('Save failed: ' + err.message, true); });
    });
    var rb = form.querySelector('.reloadBtn');
    if (rb) rb.addEventListener('click', function() {
        var page = this.closest('form').id.replace('configForm-', '');
        var loadFn = window['loadConfig_' + page];
        if (loadFn) loadFn().then(function() { showMessage('Configuration loaded'); })
            .catch(function(e) { showMessage('Load failed: ' + e.message, true); });
    });
});
"#;

const HTML_HEAD_OPEN: &str = r#"<!DOCTYPE html><html lang="en"><head><meta charset="UTF-8"><meta name="viewport" content="width=device-width,initial-scale=1.0"><title>"#;
const HTML_HEAD_TITLE_STYLE: &str = r#"</title><style>"#;
const HTML_HEAD_AFTER_STYLE: &str = r#"</style></head><body><div class="container"><header><h1>"#;
const HTML_HEAD_AFTER_H1: &str = r#"</h1><p>"#;
const HTML_SUBTITLE_NAV: &str = r#"</p></header><div class="nav">"#;
const HTML_CONTENT_OPEN: &str =
    r#"</div><div class="content"><div id="message" class="message"></div>"#;
const HTML_SCRIPT_WRAPPER_END: &str = r#"</script></body></html>"#;

const HTML_SCRIPT_MIDDLE: &str = "</script><script>";

/// Empty segment slice for unknown config groups (no allocation).
pub const EMPTY_SEGMENTS: &[&'static str] = &[];

/// One tab page: name (display + id), HTML segments, JS segments.
pub struct PageTab {
    /// Display name for the tab label.
    pub name: &'static str,
    /// HTML form body segments.
    pub html_segments: &'static [&'static str],
    /// JS segments (`loadConfig_<page>`, `saveConfig_<page>`).
    pub js_segments: &'static [&'static str],
}

/// Full config page as streamable chunks (multi-tab support).
///
/// When `pages.len() > 1`, renders a tab bar and one panel per page with lazy-loaded data.
/// Single-page configs (e.g. only "main") render without a tab bar for backward compatibility.
pub struct ConfigPageChunks {
    /// Page heading (e.g. `<h1>` content).
    pub page_heading: &'static str,
    /// Document title.
    pub title: &'static str,
    /// Subtitle under the heading.
    pub subtitle: &'static str,
    /// Left nav HTML.
    pub nav_left: &'static str,
    /// Right nav HTML.
    pub nav_right: &'static str,
    /// Extra CSS appended after the built-in stylesheet.
    pub extra_css: &'static str,
    /// Extra JavaScript appended after built-in scripts.
    pub extra_js: &'static str,
    /// Tab pages. Single page = no tab bar.
    pub pages: alloc::vec::Vec<PageTab>,
    /// Default page id (from page_to_id) to activate on load.
    pub default_page_id: alloc::string::String,
}

impl ConfigPageChunks {
    /// Stream the config page HTML to an edge-http connection.
    /// Initiates a 200 response with text/html content-type, then writes
    /// all HTML chunks. Caller must NOT call `initiate_response` beforehand.
    pub async fn write_to<T, const N: usize>(
        self,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), Error<<T as ErrorType>::Error>>
    where
        T: Read + Write,
    {
        conn.initiate_response(
            200,
            None,
            &[
                ("Content-Type", "text/html; charset=utf-8"),
                ("Connection", "close"),
            ],
        )
        .await?;

        conn.write_all(HTML_HEAD_OPEN.as_bytes()).await?;
        conn.write_all(self.title.as_bytes()).await?;
        conn.write_all(HTML_HEAD_TITLE_STYLE.as_bytes()).await?;
        conn.write_all(CONFIG_PAGE_CSS.as_bytes()).await?;
        conn.write_all(self.extra_css.as_bytes()).await?;
        conn.write_all(HTML_HEAD_AFTER_STYLE.as_bytes()).await?;
        conn.write_all(self.page_heading.as_bytes()).await?;
        conn.write_all(HTML_HEAD_AFTER_H1.as_bytes()).await?;
        conn.write_all(self.subtitle.as_bytes()).await?;
        conn.write_all(HTML_SUBTITLE_NAV.as_bytes()).await?;
        conn.write_all(self.nav_left.as_bytes()).await?;
        conn.write_all(self.nav_right.as_bytes()).await?;
        conn.write_all(HTML_CONTENT_OPEN.as_bytes()).await?;

        let show_tabs = self.pages.len() > 1;

        if show_tabs {
            let mut tab_html = String::from(r#"<div class="config-tabs">"#);
            for tab in &self.pages {
                let id = page_to_id(tab.name);
                let active = if id == self.default_page_id {
                    " config-tab active"
                } else {
                    " config-tab"
                };
                tab_html.push_str(r#"<button type="button" class=""#);
                tab_html.push_str(active);
                tab_html.push_str(r#"" data-page=""#);
                tab_html.push_str(&id);
                tab_html.push_str(r#"">"#);
                tab_html.push_str(&escape_html(tab.name));
                tab_html.push_str(r#"</button>"#);
            }
            tab_html.push_str("</div>");
            conn.write_all(tab_html.as_bytes()).await?;
        }

        for tab in &self.pages {
            let id = page_to_id(tab.name);
            let display = if id == *self.default_page_id {
                ""
            } else {
                " style=\"display:none\""
            };
            let panel_open = alloc::format!(
                r#"<div class="config-tab-panel" id="panel-{}"{}>"#,
                id,
                display
            );
            conn.write_all(panel_open.as_bytes()).await?;

            let overlay = alloc::format!(
                r#"<div class="config-loading-overlay" id="loading-{}"><span class="loading loading-overlay"></span>Loading...</div>"#,
                id
            );
            conn.write_all(overlay.as_bytes()).await?;

            let form_open = alloc::format!(r#"<form id="configForm-{}">"#, id);
            conn.write_all(form_open.as_bytes()).await?;
            for seg in tab.html_segments {
                conn.write_all(seg.as_bytes()).await?;
            }
            let form_buttons = r#"<div class="button-group"><button type="button" class="reloadBtn">Reload</button><button type="submit">Save Configuration</button></div></form>"#;
            conn.write_all(form_buttons.as_bytes()).await?;

            conn.write_all(b"</div>").await?;
        }

        conn.write_all(HTML_SCRIPT_MIDDLE.as_bytes()).await?;
        for tab in self.pages {
            for seg in tab.js_segments {
                conn.write_all(seg.as_bytes()).await?;
            }
        }

        conn.write_all(HTML_SCRIPT_MIDDLE.as_bytes()).await?;
        conn.write_all(CONFIG_PAGE_TAB_SCRIPT.as_bytes()).await?;
        let default_page_script = alloc::format!(
            "var defaultPage=\"{}\";window.addEventListener('load', function() {{ switchTab(defaultPage); }});\n",
            escape_js(&self.default_page_id)
        );
        conn.write_all(default_page_script.as_bytes()).await?;
        if !self.extra_js.is_empty() {
            conn.write_all(self.extra_js.as_bytes()).await?;
        }
        conn.write_all(HTML_SCRIPT_WRAPPER_END.as_bytes()).await?;
        // #region agent log
        debug!("config_page: all chunks written, completing response");
        // #endregion
        let result = conn.complete().await;
        // #region agent log
        match &result {
            Ok(()) => info!("config_page: response complete OK"),
            Err(_) => warn!("config_page: response complete FAILED"),
        }
        // #endregion
        result
    }
}

fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

fn escape_js(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out
}
