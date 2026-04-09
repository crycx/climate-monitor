//! Flash-backed config storage for esp-wifi-caddy.
//!
//! Key IDs for magic and format version match wifi-caddy-proc (FNV-1a of
//! `"__magic__"` and `"__format_version__"`). Values are passed in via
//! `ConfigStorageParams`.

use alloc::vec::Vec;
use core::ops::Range;

use embassy_embedded_hal::adapter::BlockingAsync;
use esp_storage::FlashStorage;
use sequential_storage::cache::NoCache;
use sequential_storage::map::MapConfig;
use sequential_storage::map::MapStorage;
use wifi_caddy::ConfigStorageParams;
use wifi_caddy::config_storage::{ConfigError, ConfigStorage};

// FNV-1a 64-bit constants (must match wifi-caddy-proc/src/utils.rs)
const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

const fn fnv1a_hash(s: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET;
    let mut i = 0;
    while i < s.len() {
        hash ^= s[i] as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
        i += 1;
    }
    hash
}

/// Key ID for magic (same as ConfigKey::Magic in wifi-caddy-proc).
const MAGIC_KEY_U64: u64 = fnv1a_hash(b"__magic__");
/// Key ID for format version (same as ConfigKey::FormatVersion in wifi-caddy-proc).
const FORMAT_VERSION_KEY_U64: u64 = fnv1a_hash(b"__format_version__");

const BUFFER_SIZE: usize = 256;

/// Flash-backed key-value storage implementing [`ConfigStorage`].
/// Use [`mount`](FlashConfigStorage::mount) with [`ConfigStorageParams`] before
/// loading or storing config.
#[doc(hidden)]
pub struct FlashConfigStorage<'d> {
    storage: MapStorage<u64, BlockingAsync<FlashStorage<'d>>, NoCache>,
    mounted: bool,
}

impl<'d> FlashConfigStorage<'d> {
    /// Create storage over the given flash range.
    pub fn new(flash: FlashStorage<'d>, range: Range<u32>) -> Self {
        Self {
            storage: MapStorage::new(
                BlockingAsync::new(flash),
                MapConfig::new(range),
                NoCache::new(),
            ),
            mounted: false,
        }
    }

    /// Mount and validate (or format) the partition using the given params.
    /// Uses hardcoded key IDs and the passed-in magic/format_version values.
    pub async fn mount(&mut self, params: &ConfigStorageParams) -> Result<(), ConfigError> {
        info!("Mounting config storage");
        let magic = match self.get_value::<u32>(MAGIC_KEY_U64).await {
            Ok(Some(magic)) => magic,
            Ok(None) => {
                error!("Config magic not found");
                self.format(params).await?;
                self.get_value::<u32>(MAGIC_KEY_U64).await?.unwrap()
            }
            Err(_) => {
                error!("Config format migration needed");
                self.format(params).await?;
                self.get_value::<u32>(MAGIC_KEY_U64).await?.unwrap()
            }
        };
        let version = self.get_value::<u32>(FORMAT_VERSION_KEY_U64).await?;
        info!("Config magic: {:x} version: {:?}", magic, version);
        if magic != params.magic {
            error!(
                "Config magic mismatch: expected {:x}, got {:x}",
                params.magic, magic
            );
            return Err(ConfigError::Backend);
        }
        self.mounted = true;
        Ok(())
    }

    /// Return whether mount has been called successfully.
    pub fn is_mounted(&self) -> bool {
        self.mounted
    }

    async fn format(&mut self, params: &ConfigStorageParams) -> Result<(), ConfigError> {
        warn!("flash_config: Formatting config storage!");
        self.storage
            .erase_all()
            .await
            .map_err(|_| ConfigError::Backend)?;
        self.set_value(MAGIC_KEY_U64, &params.magic).await?;
        self.set_value(FORMAT_VERSION_KEY_U64, &params.format_version)
            .await?;
        Ok(())
    }
}

impl ConfigStorage for FlashConfigStorage<'_> {
    async fn load_bytes(&mut self, key: u64, buf: &mut [u8]) -> Result<Option<usize>, ConfigError> {
        let mut internal_buf = [0u8; BUFFER_SIZE];
        let value: Option<Vec<u8>> = self
            .storage
            .fetch_item(&mut internal_buf, &key)
            .await
            .map_err(|_| ConfigError::Backend)?;
        match value {
            Some(v) => {
                let len = v.len().min(buf.len());
                buf[..len].copy_from_slice(&v[..len]);
                Ok(Some(len))
            }
            None => Ok(None),
        }
    }

    async fn store_bytes(&mut self, key: u64, bytes: &[u8]) -> Result<(), ConfigError> {
        let mut buffer = [0u8; BUFFER_SIZE];
        let current: Option<Vec<u8>> = self
            .storage
            .fetch_item(&mut buffer, &key)
            .await
            .map_err(|_| ConfigError::Backend)?;

        if let Some(ref existing) = current {
            if existing.len() == bytes.len() && existing.as_slice() == bytes {
                return Ok(());
            }
        }

        let value: Vec<u8> = bytes.to_vec();
        self.storage
            .store_item(&mut buffer, &key, &value)
            .await
            .map_err(|_| ConfigError::Backend)
    }
}
