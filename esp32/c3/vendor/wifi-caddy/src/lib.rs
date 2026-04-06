#![doc = include_str!("../README.md")]
#![no_std]
#![warn(missing_docs)]
#![allow(async_fn_in_trait)]
#![cfg_attr(feature = "nightly", feature(impl_trait_in_assoc_type))]

extern crate alloc;

mod fmt;

pub mod config_storage;

#[cfg(feature = "portal")]
pub mod portal;
#[cfg(feature = "portal")]
mod run_http;

#[cfg(all(feature = "portal", feature = "debug-server"))]
pub use run_http::run_http_debug_loop;
#[cfg(feature = "portal")]
pub use run_http::{ConfigUiOptions, run_http_config_loop};

/// Parameters for config storage mount/format. Only the values are configurable;
/// key IDs are fixed to match wifi-caddy-proc.
#[doc(hidden)]
#[derive(Clone, Copy, Debug)]
pub struct ConfigStorageParams {
    /// Magic value stored at the magic key (format identifier).
    pub magic: u32,
    /// Format version stored at the format version key.
    pub format_version: u32,
}

/// Handle returned by the platform-specific init macro (e.g. `esp_wifi_caddy::wifi_init!`).
/// Use [`.config()`](ConfigHandle::config) to get the shared config mutex to pass into
/// application tasks.
pub struct ConfigHandle<C: 'static> {
    config: &'static embassy_sync::mutex::Mutex<
        embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
        C,
    >,
}

impl<C: 'static> ConfigHandle<C> {
    /// Create a new `ConfigHandle` wrapping the given config mutex.
    pub fn new(
        config: &'static embassy_sync::mutex::Mutex<
            embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
            C,
        >,
    ) -> Self {
        Self { config }
    }

    /// Returns the shared config mutex (`'static`), for use in tasks or elsewhere.
    pub fn config(
        &self,
    ) -> &'static embassy_sync::mutex::Mutex<
        embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
        C,
    > {
        self.config
    }
}
