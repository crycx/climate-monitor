extern crate alloc;

use alloc::{string::String, vec::Vec};
use core::fmt;
use core::ops::Range;

use embassy_embedded_hal::adapter::BlockingAsync;
use esp_bootloader_esp_idf::partitions;
use esp_storage::FlashStorage;
use log::warn;
use sequential_storage::cache::NoCache;
use sequential_storage::map::{MapConfig, MapStorage};
use serde::{Deserialize, Serialize};

const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;
const STORAGE_MAGIC: u32 = 0x4255_aa42;
const STORAGE_VERSION: u32 = 1;
const BUFFER_SIZE: usize = 256;

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

const MAGIC_KEY: u64 = fnv1a_hash(b"__magic__");
const FORMAT_VERSION_KEY: u64 = fnv1a_hash(b"__format_version__");
const WIFI_SSID_KEY: u64 = fnv1a_hash(b"wifi_ssid");
const WIFI_PASSWORD_KEY: u64 = fnv1a_hash(b"wifi_password");
const WIFI_LOCATION_KEY: u64 = fnv1a_hash(b"wifi_location");
const API_URL_KEY: u64 = fnv1a_hash(b"api_url");
const API_KEY_KEY: u64 = fnv1a_hash(b"api_key");
const WIFI_SSID_2_KEY: u64 = fnv1a_hash(b"wifi_ssid_2");
const WIFI_PASSWORD_2_KEY: u64 = fnv1a_hash(b"wifi_password_2");
const WIFI_LOCATION_2_KEY: u64 = fnv1a_hash(b"wifi_location_2");
const WIFI_SSID_3_KEY: u64 = fnv1a_hash(b"wifi_ssid_3");
const WIFI_PASSWORD_3_KEY: u64 = fnv1a_hash(b"wifi_password_3");
const WIFI_LOCATION_3_KEY: u64 = fnv1a_hash(b"wifi_location_3");
const WIFI_SSID_4_KEY: u64 = fnv1a_hash(b"wifi_ssid_4");
const WIFI_PASSWORD_4_KEY: u64 = fnv1a_hash(b"wifi_password_4");
const WIFI_LOCATION_4_KEY: u64 = fnv1a_hash(b"wifi_location_4");
const WIFI_SSID_5_KEY: u64 = fnv1a_hash(b"wifi_ssid_5");
const WIFI_PASSWORD_5_KEY: u64 = fnv1a_hash(b"wifi_password_5");
const WIFI_LOCATION_5_KEY: u64 = fnv1a_hash(b"wifi_location_5");
const TEMP_OFFSET_KEY: u64 = fnv1a_hash(b"temp_offset");
const HUM_OFFSET_KEY: u64 = fnv1a_hash(b"hum_offset");
const OLED_ORIENTATION_KEY: u64 = fnv1a_hash(b"oled_orientation");

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppConfig {
    pub wifi_ssid: String,
    pub wifi_password: String,
    pub wifi_location: String,
    pub api_url: String,
    pub api_key: String,
    pub wifi_ssid_2: String,
    pub wifi_password_2: String,
    pub wifi_location_2: String,
    pub wifi_ssid_3: String,
    pub wifi_password_3: String,
    pub wifi_location_3: String,
    pub wifi_ssid_4: String,
    pub wifi_password_4: String,
    pub wifi_location_4: String,
    pub wifi_ssid_5: String,
    pub wifi_password_5: String,
    pub wifi_location_5: String,
    pub temp_offset: String,
    pub hum_offset: String,
    pub oled_orientation: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            wifi_ssid: option_env!("WIFI_SSID").unwrap_or("").into(),
            wifi_password: option_env!("WIFI_PASSWORD").unwrap_or("").into(),
            wifi_location: option_env!("WIFI_LOCATION").unwrap_or("").into(),
            api_url: option_env!("API_URL").unwrap_or("").into(),
            api_key: option_env!("API_KEY").unwrap_or("").into(),
            wifi_ssid_2: option_env!("WIFI_SSID_2").unwrap_or("").into(),
            wifi_password_2: option_env!("WIFI_PASSWORD_2").unwrap_or("").into(),
            wifi_location_2: option_env!("WIFI_LOCATION_2").unwrap_or("").into(),
            wifi_ssid_3: option_env!("WIFI_SSID_3").unwrap_or("").into(),
            wifi_password_3: option_env!("WIFI_PASSWORD_3").unwrap_or("").into(),
            wifi_location_3: option_env!("WIFI_LOCATION_3").unwrap_or("").into(),
            wifi_ssid_4: option_env!("WIFI_SSID_4").unwrap_or("").into(),
            wifi_password_4: option_env!("WIFI_PASSWORD_4").unwrap_or("").into(),
            wifi_location_4: option_env!("WIFI_LOCATION_4").unwrap_or("").into(),
            wifi_ssid_5: option_env!("WIFI_SSID_5").unwrap_or("").into(),
            wifi_password_5: option_env!("WIFI_PASSWORD_5").unwrap_or("").into(),
            wifi_location_5: option_env!("WIFI_LOCATION_5").unwrap_or("").into(),
            temp_offset: option_env!("TEMP_OFFSET").unwrap_or("-1.7").into(),
            hum_offset: option_env!("HUM_OFFSET").unwrap_or("-7.0").into(),
            oled_orientation: option_env!("OLED_ORIENTATION").unwrap_or("landscape").into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    BufferTooSmall(usize),
    Utf8,
    Backend,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BufferTooSmall(n) => write!(f, "buffer too small: need {} bytes", n),
            Self::Utf8 => write!(f, "invalid UTF-8"),
            Self::Backend => write!(f, "backend storage error"),
        }
    }
}

pub struct FlashConfigStorage<'d> {
    storage: MapStorage<u64, BlockingAsync<FlashStorage<'d>>, NoCache>,
    mounted: bool,
}

impl<'d> FlashConfigStorage<'d> {
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

    pub async fn mount(&mut self) -> Result<(), ConfigError> {
        let magic = match self.get_u32(MAGIC_KEY).await {
            Ok(Some(magic)) => magic,
            Ok(None) => {
                warn!("config: magic missing, formatting config storage");
                self.format().await?;
                self.get_u32(MAGIC_KEY).await?.unwrap_or(STORAGE_MAGIC)
            }
            Err(_) => {
                warn!("config: storage unreadable, formatting config storage");
                self.format().await?;
                self.get_u32(MAGIC_KEY).await?.unwrap_or(STORAGE_MAGIC)
            }
        };

        let version = self.get_u32(FORMAT_VERSION_KEY).await?;
        if magic != STORAGE_MAGIC || version != Some(STORAGE_VERSION) {
            warn!("config: magic/version mismatch, formatting config storage");
            self.format().await?;
        }

        self.mounted = true;
        Ok(())
    }

    pub async fn load_config(&mut self) -> Result<AppConfig, ConfigError> {
        let mut cfg = AppConfig::default();
        if let Some(value) = self.get_string(WIFI_SSID_KEY).await? {
            cfg.wifi_ssid = value;
        }
        if let Some(value) = self.get_string(WIFI_PASSWORD_KEY).await? {
            cfg.wifi_password = value;
        }
        if let Some(value) = self.get_string(WIFI_LOCATION_KEY).await? {
            cfg.wifi_location = value;
        }
        if let Some(value) = self.get_string(API_URL_KEY).await? {
            cfg.api_url = value;
        }
        if let Some(value) = self.get_string(API_KEY_KEY).await? {
            cfg.api_key = value;
        }
        if let Some(value) = self.get_string(WIFI_SSID_2_KEY).await? {
            cfg.wifi_ssid_2 = value;
        }
        if let Some(value) = self.get_string(WIFI_PASSWORD_2_KEY).await? {
            cfg.wifi_password_2 = value;
        }
        if let Some(value) = self.get_string(WIFI_LOCATION_2_KEY).await? {
            cfg.wifi_location_2 = value;
        }
        if let Some(value) = self.get_string(WIFI_SSID_3_KEY).await? {
            cfg.wifi_ssid_3 = value;
        }
        if let Some(value) = self.get_string(WIFI_PASSWORD_3_KEY).await? {
            cfg.wifi_password_3 = value;
        }
        if let Some(value) = self.get_string(WIFI_LOCATION_3_KEY).await? {
            cfg.wifi_location_3 = value;
        }
        if let Some(value) = self.get_string(WIFI_SSID_4_KEY).await? {
            cfg.wifi_ssid_4 = value;
        }
        if let Some(value) = self.get_string(WIFI_PASSWORD_4_KEY).await? {
            cfg.wifi_password_4 = value;
        }
        if let Some(value) = self.get_string(WIFI_LOCATION_4_KEY).await? {
            cfg.wifi_location_4 = value;
        }
        if let Some(value) = self.get_string(WIFI_SSID_5_KEY).await? {
            cfg.wifi_ssid_5 = value;
        }
        if let Some(value) = self.get_string(WIFI_PASSWORD_5_KEY).await? {
            cfg.wifi_password_5 = value;
        }
        if let Some(value) = self.get_string(WIFI_LOCATION_5_KEY).await? {
            cfg.wifi_location_5 = value;
        }
        if let Some(value) = self.get_string(TEMP_OFFSET_KEY).await? {
            cfg.temp_offset = value;
        }
        if let Some(value) = self.get_string(HUM_OFFSET_KEY).await? {
            cfg.hum_offset = value;
        }
        if let Some(value) = self.get_string(OLED_ORIENTATION_KEY).await? {
            cfg.oled_orientation = value;
        }
        Ok(cfg)
    }

    pub async fn store_config(&mut self, cfg: &AppConfig) -> Result<(), ConfigError> {
        self.set_u32(MAGIC_KEY, STORAGE_MAGIC).await?;
        self.set_u32(FORMAT_VERSION_KEY, STORAGE_VERSION).await?;
        self.set_string(WIFI_SSID_KEY, &cfg.wifi_ssid).await?;
        self.set_string(WIFI_PASSWORD_KEY, &cfg.wifi_password).await?;
        self.set_string(WIFI_LOCATION_KEY, &cfg.wifi_location).await?;
        self.set_string(API_URL_KEY, &cfg.api_url).await?;
        self.set_string(API_KEY_KEY, &cfg.api_key).await?;
        self.set_string(WIFI_SSID_2_KEY, &cfg.wifi_ssid_2).await?;
        self.set_string(WIFI_PASSWORD_2_KEY, &cfg.wifi_password_2).await?;
        self.set_string(WIFI_LOCATION_2_KEY, &cfg.wifi_location_2).await?;
        self.set_string(WIFI_SSID_3_KEY, &cfg.wifi_ssid_3).await?;
        self.set_string(WIFI_PASSWORD_3_KEY, &cfg.wifi_password_3).await?;
        self.set_string(WIFI_LOCATION_3_KEY, &cfg.wifi_location_3).await?;
        self.set_string(WIFI_SSID_4_KEY, &cfg.wifi_ssid_4).await?;
        self.set_string(WIFI_PASSWORD_4_KEY, &cfg.wifi_password_4).await?;
        self.set_string(WIFI_LOCATION_4_KEY, &cfg.wifi_location_4).await?;
        self.set_string(WIFI_SSID_5_KEY, &cfg.wifi_ssid_5).await?;
        self.set_string(WIFI_PASSWORD_5_KEY, &cfg.wifi_password_5).await?;
        self.set_string(WIFI_LOCATION_5_KEY, &cfg.wifi_location_5).await?;
        self.set_string(TEMP_OFFSET_KEY, &cfg.temp_offset).await?;
        self.set_string(HUM_OFFSET_KEY, &cfg.hum_offset).await?;
        self.set_string(OLED_ORIENTATION_KEY, &cfg.oled_orientation)
            .await?;
        Ok(())
    }

    async fn format(&mut self) -> Result<(), ConfigError> {
        self.storage
            .erase_all()
            .await
            .map_err(|_| ConfigError::Backend)?;
        self.set_u32(MAGIC_KEY, STORAGE_MAGIC).await?;
        self.set_u32(FORMAT_VERSION_KEY, STORAGE_VERSION).await?;
        Ok(())
    }

    async fn get_u32(&mut self, key: u64) -> Result<Option<u32>, ConfigError> {
        let mut buffer = [0u8; BUFFER_SIZE];
        self.storage
            .fetch_item(&mut buffer, &key)
            .await
            .map_err(|_| ConfigError::Backend)
    }

    async fn set_u32(&mut self, key: u64, value: u32) -> Result<(), ConfigError> {
        let mut buffer = [0u8; BUFFER_SIZE];
        self.storage
            .store_item(&mut buffer, &key, &value)
            .await
            .map_err(|_| ConfigError::Backend)
    }

    async fn get_string(&mut self, key: u64) -> Result<Option<String>, ConfigError> {
        let mut buffer = [0u8; BUFFER_SIZE];
        let value: Option<Vec<u8>> = self
            .storage
            .fetch_item(&mut buffer, &key)
            .await
            .map_err(|_| ConfigError::Backend)?;

        match value {
            Some(bytes) => core::str::from_utf8(&bytes)
                .map(|s| Some(String::from(s)))
                .map_err(|_| ConfigError::Utf8),
            None => Ok(None),
        }
    }

    async fn set_string(&mut self, key: u64, value: &str) -> Result<(), ConfigError> {
        if value.len() > BUFFER_SIZE {
            return Err(ConfigError::BufferTooSmall(value.len()));
        }

        let mut buffer = [0u8; BUFFER_SIZE];
        let current: Option<Vec<u8>> = self
            .storage
            .fetch_item(&mut buffer, &key)
            .await
            .map_err(|_| ConfigError::Backend)?;

        if let Some(existing) = current {
            if existing.as_slice() == value.as_bytes() {
                return Ok(());
            }
        }

        let bytes = value.as_bytes().to_vec();
        self.storage
            .store_item(&mut buffer, &key, &bytes)
            .await
            .map_err(|_| ConfigError::Backend)
    }
}

pub fn resolve_partition_range(
    flash: &mut FlashStorage<'static>,
    partition_name: &str,
) -> Result<Range<u32>, ConfigError> {
    let mut buffer = [0u8; partitions::PARTITION_TABLE_MAX_LEN];
    let partition_table = partitions::read_partition_table(flash, &mut buffer)
        .map_err(|_| ConfigError::Backend)?;
    let part = partition_table
        .iter()
        .find(|p| p.label_as_str() == partition_name)
        .ok_or(ConfigError::Backend)?;
    Ok(part.offset()..(part.offset() + part.len()))
}