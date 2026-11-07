//! Node settings: build-time defaults from `.cargo/config.toml`, overridden
//! by a record saved in the `config` flash partition (USB console or WiFi
//! portal).
//!
//! Record layout (little endian, `RECORD_LEN` bytes, start of the partition):
//!
//! ```text
//! 0..4     magic "CMNC"
//! 4        version
//! 5        flags: bit0 external antenna, bit1 battery ADC
//! 8..12    interval_secs   u32
//! 12..16   temp_offset     f32
//! 16..20   hum_offset      f32
//! 20..24   dht_warmup_ms   u32
//! 24..28   battery_divider f32
//! 28       name_len, 29..53 name
//! 53       key_len,  54..118 key
//! 124..128 CRC-32 of bytes 0..124
//! ```

use alloc::{format, string::String, vec::Vec};
use core::ops::Range;

use embedded_storage::{ReadStorage, Storage};
use esp_bootloader_esp_idf::partitions;
use esp_storage::FlashStorage;

use crate::espnow_packet::MAX_NAME_LEN;

pub const MAX_KEY_LEN: usize = 64;
pub const MIN_KEY_LEN: usize = 8;
pub const MIN_INTERVAL_SECS: u32 = 10;
pub const MAX_INTERVAL_SECS: u32 = 6 * 3600;
const MAX_WARMUP_MS: u32 = 5000;
const MAX_OFFSET: f32 = 20.0;

const PARTITION_LABEL: &str = "config";
const MAGIC: [u8; 4] = *b"CMNC";
const VERSION: u8 = 1;
const RECORD_LEN: usize = 128;
const CRC_OFFSET: usize = RECORD_LEN - 4;
const FLAG_EXTERNAL_ANTENNA: u8 = 0x01;
const FLAG_BATTERY_ADC: u8 = 0x02;

#[derive(Clone, PartialEq)]
pub struct NodeConfig {
    pub name: String,
    pub interval_secs: u32,
    pub temp_offset: f32,
    pub hum_offset: f32,
    pub key: String,
    pub external_antenna: bool,
    pub dht_warmup_ms: u32,
    pub battery_adc: bool,
    pub battery_divider: f32,
}

fn parse_bool(value: &str) -> Option<bool> {
    let v = value.trim();
    if ["1", "true", "yes", "on"].iter().any(|t| v.eq_ignore_ascii_case(t)) {
        Some(true)
    } else if ["0", "false", "no", "off"].iter().any(|t| v.eq_ignore_ascii_case(t)) {
        Some(false)
    } else {
        None
    }
}

impl NodeConfig {
    /// Build-time defaults; invalid values fall back to safe ones.
    pub fn defaults() -> Self {
        let mut cfg = Self {
            name: String::new(),
            interval_secs: 300,
            temp_offset: 0.0,
            hum_offset: 0.0,
            key: String::new(),
            external_antenna: false,
            dht_warmup_ms: 1500,
            battery_adc: true,
            battery_divider: 2.0,
        };
        let env: [(&str, Option<&str>); 9] = [
            ("name", option_env!("SENSOR_NAME")),
            ("interval", option_env!("SENSOR_INTERVAL_SECS")),
            ("temp_offset", option_env!("TEMP_OFFSET")),
            ("hum_offset", option_env!("HUM_OFFSET")),
            ("key", option_env!("ESPNOW_KEY")),
            ("antenna", option_env!("ANTENNA")),
            ("warmup", option_env!("DHT22_WARMUP_MS")),
            ("battery", option_env!("BATTERY_ADC")),
            ("divider", option_env!("BATTERY_DIVIDER")),
        ];
        for (field, value) in env {
            if let Some(value) = value {
                let _ = cfg.set(field, value);
            }
        }
        cfg
    }

    pub fn has_key(&self) -> bool {
        self.key.len() >= MIN_KEY_LEN
    }

    /// Sets one field from text. Shared by the USB console and the portal.
    pub fn set(&mut self, field: &str, value: &str) -> Result<(), String> {
        let value = value.trim();
        match field {
            "name" => {
                if value.len() > MAX_NAME_LEN {
                    return Err(format!("name: max {} bytes", MAX_NAME_LEN));
                }
                self.name = value.into();
            }
            "interval" => {
                let secs: u32 = value.parse().map_err(|_| String::from("interval: whole seconds"))?;
                if !(MIN_INTERVAL_SECS..=MAX_INTERVAL_SECS).contains(&secs) {
                    return Err(format!(
                        "interval: {}..{} s",
                        MIN_INTERVAL_SECS, MAX_INTERVAL_SECS
                    ));
                }
                self.interval_secs = secs;
            }
            "temp_offset" | "hum_offset" => {
                let v: f32 = value.parse().map_err(|_| format!("{}: number", field))?;
                if !v.is_finite() || v.abs() > MAX_OFFSET {
                    return Err(format!("{}: -{}..{}", field, MAX_OFFSET, MAX_OFFSET));
                }
                if field == "temp_offset" {
                    self.temp_offset = v;
                } else {
                    self.hum_offset = v;
                }
            }
            "key" => {
                if value.len() < MIN_KEY_LEN || value.len() > MAX_KEY_LEN {
                    return Err(format!("key: {}..{} characters", MIN_KEY_LEN, MAX_KEY_LEN));
                }
                if value.chars().any(|c| c.is_whitespace() || !c.is_ascii()) {
                    return Err("key: ASCII, no spaces".into());
                }
                self.key = value.into();
            }
            "antenna" => {
                self.external_antenna = match value {
                    v if v.eq_ignore_ascii_case("external") => true,
                    v if v.eq_ignore_ascii_case("internal") => false,
                    _ => return Err("antenna: internal|external".into()),
                };
            }
            "warmup" => {
                let ms: u32 = value.parse().map_err(|_| String::from("warmup: milliseconds"))?;
                if ms > MAX_WARMUP_MS {
                    return Err(format!("warmup: 0..{} ms", MAX_WARMUP_MS));
                }
                self.dht_warmup_ms = ms;
            }
            "battery" => {
                self.battery_adc = parse_bool(value).ok_or_else(|| String::from("battery: on|off"))?;
            }
            "divider" => {
                let v: f32 = value.parse().map_err(|_| String::from("divider: number"))?;
                if !(1.0..=10.0).contains(&v) {
                    return Err("divider: 1..10".into());
                }
                self.battery_divider = v;
            }
            _ => return Err(format!("unknown setting '{}'", field)),
        }
        Ok(())
    }

    /// `key=value` lines for the console / web page. The key itself is never shown.
    pub fn describe(&self) -> Vec<(&'static str, String)> {
        alloc::vec![
            ("name", self.name.clone()),
            ("interval", format!("{}", self.interval_secs)),
            ("temp_offset", format!("{:.1}", self.temp_offset)),
            ("hum_offset", format!("{:.1}", self.hum_offset)),
            ("key_set", String::from(if self.has_key() { "yes" } else { "no" })),
            ("antenna", String::from(if self.external_antenna { "external" } else { "internal" })),
            ("warmup", format!("{}", self.dht_warmup_ms)),
            ("battery", String::from(if self.battery_adc { "on" } else { "off" })),
            ("divider", format!("{:.2}", self.battery_divider)),
        ]
    }

    fn encode(&self) -> [u8; RECORD_LEN] {
        let mut out = [0u8; RECORD_LEN];
        out[0..4].copy_from_slice(&MAGIC);
        out[4] = VERSION;
        let mut flags = 0;
        if self.external_antenna {
            flags |= FLAG_EXTERNAL_ANTENNA;
        }
        if self.battery_adc {
            flags |= FLAG_BATTERY_ADC;
        }
        out[5] = flags;
        out[8..12].copy_from_slice(&self.interval_secs.to_le_bytes());
        out[12..16].copy_from_slice(&self.temp_offset.to_le_bytes());
        out[16..20].copy_from_slice(&self.hum_offset.to_le_bytes());
        out[20..24].copy_from_slice(&self.dht_warmup_ms.to_le_bytes());
        out[24..28].copy_from_slice(&self.battery_divider.to_le_bytes());
        let name = &self.name.as_bytes()[..self.name.len().min(MAX_NAME_LEN)];
        out[28] = name.len() as u8;
        out[29..29 + name.len()].copy_from_slice(name);
        let key = &self.key.as_bytes()[..self.key.len().min(MAX_KEY_LEN)];
        out[53] = key.len() as u8;
        out[54..54 + key.len()].copy_from_slice(key);
        let crc = crc32(&out[..CRC_OFFSET]);
        out[CRC_OFFSET..].copy_from_slice(&crc.to_le_bytes());
        out
    }

    fn decode(data: &[u8; RECORD_LEN]) -> Option<Self> {
        if data[0..4] != MAGIC || data[4] != VERSION {
            return None;
        }
        let crc = u32::from_le_bytes([data[124], data[125], data[126], data[127]]);
        if crc != crc32(&data[..CRC_OFFSET]) {
            return None;
        }
        let u32_at = |i: usize| u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
        let f32_at = |i: usize| f32::from_bits(u32_at(i));
        let name_len = (data[28] as usize).min(MAX_NAME_LEN);
        let key_len = (data[53] as usize).min(MAX_KEY_LEN);
        let name = core::str::from_utf8(&data[29..29 + name_len]).ok()?;
        let key = core::str::from_utf8(&data[54..54 + key_len]).ok()?;

        // Re-validate through `set` so a corrupt-but-CRC-valid record cannot
        // smuggle in out-of-range values.
        let mut cfg = Self::defaults();
        let mut apply = |field: &str, value: &str| cfg.set(field, value).ok();
        apply("name", name)?;
        apply("interval", &format!("{}", u32_at(8)))?;
        apply("temp_offset", &format!("{}", f32_at(12)))?;
        apply("hum_offset", &format!("{}", f32_at(16)))?;
        apply("warmup", &format!("{}", u32_at(20)))?;
        apply("divider", &format!("{}", f32_at(24)))?;
        if !key.is_empty() {
            apply("key", key)?;
        }
        cfg.external_antenna = data[5] & FLAG_EXTERNAL_ANTENNA != 0;
        cfg.battery_adc = data[5] & FLAG_BATTERY_ADC != 0;
        Some(cfg)
    }
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

pub struct ConfigStore {
    flash: FlashStorage<'static>,
    range: Option<Range<u32>>,
}

impl ConfigStore {
    pub fn new(mut flash: FlashStorage<'static>) -> Self {
        let mut buffer = [0u8; partitions::PARTITION_TABLE_MAX_LEN];
        let range = partitions::read_partition_table(&mut flash, &mut buffer)
            .ok()
            .and_then(|table| {
                table
                    .iter()
                    .find(|p| p.label_as_str() == PARTITION_LABEL)
                    .map(|p| p.offset()..p.offset() + p.len())
            })
            .filter(|r| (r.end - r.start) as usize >= RECORD_LEN);
        Self { flash, range }
    }

    pub fn available(&self) -> bool {
        self.range.is_some()
    }

    /// Saved settings, or build-time defaults when nothing valid is stored.
    pub fn load(&mut self) -> (NodeConfig, bool) {
        if let Some(range) = &self.range {
            let mut record = [0u8; RECORD_LEN];
            if self.flash.read(range.start, &mut record).is_ok() {
                if let Some(cfg) = NodeConfig::decode(&record) {
                    return (cfg, true);
                }
            }
        }
        (NodeConfig::defaults(), false)
    }

    pub fn save(&mut self, cfg: &NodeConfig) -> Result<(), &'static str> {
        let range = self.range.as_ref().ok_or("no 'config' partition (flash with partitions.csv)")?;
        let record = cfg.encode();
        self.flash.write(range.start, &record).map_err(|_| "flash write failed")?;
        // Read back to catch silent write failures.
        let mut check = [0u8; RECORD_LEN];
        self.flash.read(range.start, &mut check).map_err(|_| "flash read failed")?;
        if check != record {
            return Err("flash verify failed");
        }
        Ok(())
    }

    /// Erase the saved record so build-time defaults apply again.
    pub fn clear(&mut self) -> Result<(), &'static str> {
        let range = self.range.as_ref().ok_or("no 'config' partition")?;
        self.flash.write(range.start, &[0xFFu8; RECORD_LEN]).map_err(|_| "flash write failed")
    }
}
