//! Generic config-group GET/POST handler and shared types.
//!
//! Use with a config type that implements `ConfigApi` and `ConfigLoadStore`;
//! storage is passed as a second mutex and must implement `ConfigStorage`.

use alloc::string::String;

use crate::config_storage::{ConfigApi, ConfigChangedSet, ConfigLoadStore, ConfigStorage};
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::mutex::Mutex;
use serde::Deserialize;

/// Query for config-group endpoints: optional `set` body (JSON string).
#[derive(Deserialize)]
pub struct ConfigQuery {
    /// If present, apply this JSON to the config group and persist.
    pub set: Option<String>,
}

/// Result from the config-group handler: either JSON success or an error.
pub enum ConfigGroupResult {
    /// 200 with JSON body.
    Json(String),
    /// Error with HTTP status code and message.
    Err(u16, String),
}

/// Handle GET (return group JSON) or POST with `?set=...` (apply JSON and persist).
///
/// Locks config; if `query.set` is some, calls `set_group_json` to get the set of
/// actually changed variants; only then persists (store_to) when the set is non-empty,
/// and if `on_updated` is provided calls it with that set. Returns group JSON.
pub async fn handle_config_group<R, C, S, F>(
    config: &Mutex<R, C>,
    io: &Mutex<R, S>,
    group: &str,
    query: ConfigQuery,
    buf: &mut [u8],
    on_updated: Option<F>,
) -> ConfigGroupResult
where
    R: RawMutex,
    C: ConfigApi + ConfigLoadStore,
    S: ConfigStorage,
    F: Fn(C::ChangedSet),
{
    let mut notify_changed = None;

    if let Some(ref set_json) = query.set {
        let mut config_guard = config.lock().await;
        let changed = match config_guard.set_group_json(group, set_json) {
            Ok(c) => c,
            Err(e) => {
                error!(
                    "handle_config_group: set_group_json failed for group {}: {}",
                    group,
                    crate::fmt::DisplayFmt(&e)
                );
                return ConfigGroupResult::Err(400, alloc::format!("{}", e));
            }
        };
        if !changed.is_empty() {
            let mut io_guard = io.lock().await;
            if let Err(e) = config_guard.store_to(&mut *io_guard).await {
                error!(
                    "handle_config_group: store_to failed for group {}: {}",
                    group,
                    crate::fmt::DisplayFmt(&e)
                );
                return ConfigGroupResult::Err(500, alloc::format!("{}", e));
            }
            notify_changed = Some(changed);
        }
    }

    if let (Some(changed), Some(ref f)) = (notify_changed, on_updated) {
        debug!("handle_config_group: calling on_updated");
        f(changed);
        debug!("handle_config_group: on_updated returned");
    }

    let config_guard = config.lock().await;
    match config_guard.get_group_json(group, buf) {
        Ok(len) => {
            let json_str = String::from(core::str::from_utf8(&buf[..len]).unwrap_or(""));
            ConfigGroupResult::Json(json_str)
        }
        Err(e) => ConfigGroupResult::Err(400, alloc::format!("{}", e)),
    }
}
