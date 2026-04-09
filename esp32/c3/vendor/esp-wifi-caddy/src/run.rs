//! High-level API: WiFi + flash config storage + HTTP config UI on AP.
//!
//! Use `esp_wifi_caddy::wifi_init!` to initialize WiFi, mount config from a flash
//! partition, load the config, and start the HTTP config UI on the AP stack.

use alloc::boxed::Box;
use core::ops::Range;

#[cfg(feature = "partition-table")]
use esp_bootloader_esp_idf::partitions;

use embassy_executor::Spawner;
use embassy_net::Stack;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use esp_hal::peripherals::WIFI;
use esp_storage::FlashStorage;
use wifi_caddy::config_storage::{ConfigApi, ConfigFormGen, ConfigGet, ConfigLoadStore};
use wifi_caddy::{ConfigHandle, ConfigStorageParams, ConfigUiOptions};

use crate::flash_config::FlashConfigStorage;
use crate::{WifiCommandSender, WifiStacks, init};

#[doc(hidden)]
pub async fn run_inner<C, F>(
    spawner: Spawner,
    wifi: WIFI<'static>,
    flash: FlashStorage<'static>,
    partition_range: Range<u32>,
    params: ConfigStorageParams,
    ui: ConfigUiOptions,
    on_updated: Option<&'static (dyn Fn(C::ChangedSet) + Send)>,
    spawn_workers: F,
) -> Result<(WifiStacks, WifiCommandSender, ConfigHandle<C>), wifi_caddy::config_storage::ConfigError>
where
    C: ConfigFormGen + ConfigGet + ConfigApi + ConfigLoadStore + Send + 'static,
    C::ChangedSet: Send,
    F: FnOnce(
        Spawner,
        Stack<'static>,
        Stack<'static>,
        &'static Mutex<CriticalSectionRawMutex, C>,
        &'static Mutex<CriticalSectionRawMutex, FlashConfigStorage<'static>>,
        ConfigUiOptions,
        Option<&'static (dyn Fn(C::ChangedSet) + Send)>,
    ),
{
    let mut storage = FlashConfigStorage::new(flash, partition_range);
    storage.mount(&params).await?;
    let config = C::load_from(&mut storage).await?;

    let config_mutex: &'static Mutex<CriticalSectionRawMutex, C> =
        Box::leak(Box::new(Mutex::new(config)));
    let io_mutex: &'static Mutex<CriticalSectionRawMutex, FlashConfigStorage<'static>> =
        Box::leak(Box::new(Mutex::new(storage)));

    let (wifi_stacks, wifi_sender) = init(&spawner, wifi).await;
    debug!("wifi initialized (STA + AP)");

    let sta_stack = wifi_stacks.sta;
    wifi_caddy::portal::start(spawner, wifi_stacks.ap, move |s, ap_stack| {
        spawn_workers(
            s,
            ap_stack,
            sta_stack,
            config_mutex,
            io_mutex,
            ui,
            on_updated,
        );
    });

    Ok((wifi_stacks, wifi_sender, ConfigHandle::new(config_mutex)))
}

#[doc(hidden)]
#[cfg(feature = "partition-table")]
pub fn resolve_partition_range(
    flash: &mut FlashStorage<'static>,
    partition_name: &str,
) -> Result<Range<u32>, wifi_caddy::config_storage::ConfigError> {
    let mut buffer = [0u8; partitions::PARTITION_TABLE_MAX_LEN];
    let partition_table = partitions::read_partition_table(flash, &mut buffer)
        .map_err(|_| wifi_caddy::config_storage::ConfigError::Backend)?;
    let part = partition_table
        .iter()
        .find(|p| p.label_as_str() == partition_name)
        .ok_or(wifi_caddy::config_storage::ConfigError::Backend)?;
    Ok(part.offset()..(part.offset() + part.len()))
}

#[doc(hidden)]
#[cfg(feature = "partition-table")]
pub async fn run_inner_by_partition<C, F>(
    spawner: Spawner,
    wifi: WIFI<'static>,
    mut flash: FlashStorage<'static>,
    partition_name: &str,
    params: ConfigStorageParams,
    ui: ConfigUiOptions,
    on_updated: Option<&'static (dyn Fn(C::ChangedSet) + Send)>,
    spawn_workers: F,
) -> Result<(WifiStacks, WifiCommandSender, ConfigHandle<C>), wifi_caddy::config_storage::ConfigError>
where
    C: ConfigFormGen + ConfigGet + ConfigApi + ConfigLoadStore + Send + 'static,
    C::ChangedSet: Send,
    F: FnOnce(
        Spawner,
        Stack<'static>,
        Stack<'static>,
        &'static Mutex<CriticalSectionRawMutex, C>,
        &'static Mutex<CriticalSectionRawMutex, FlashConfigStorage<'static>>,
        ConfigUiOptions,
        Option<&'static (dyn Fn(C::ChangedSet) + Send)>,
    ),
{
    let partition_range = resolve_partition_range(&mut flash, partition_name)?;
    run_inner(
        spawner,
        wifi,
        flash,
        partition_range,
        params,
        ui,
        on_updated,
        spawn_workers,
    )
    .await
}
