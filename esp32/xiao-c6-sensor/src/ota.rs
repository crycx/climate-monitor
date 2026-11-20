//! Firmware update over ESP-NOW, sensor side.
//!
//! The gateway sets `SET_OTA` in its Config reply when Home Assistant asked
//! for an update. The sensor then pulls the image: it asks for the OtaInfo
//! (size, SHA-256, version), then for each chunk in turn (stop and wait;
//! a lost request or reply is simply asked for again), and writes it to the
//! OTA slot it is not running from. Only after the SHA-256 of what is in
//! flash matches does it select that slot in `otadata` and restart.
//!
//! The ESP-IDF bootloader that espflash installs has no rollback: firmware
//! that starts but cannot send stays until it is flashed over USB. Before
//! switching, the image is checked to be an ESP32-C6 app of this project.

use core::sync::atomic::{AtomicBool, Ordering};

use embassy_time::{Duration, Instant, Timer, with_timeout};
use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use esp_bootloader_esp_idf::ota::Ota;
use esp_bootloader_esp_idf::partitions::{
    self, AppPartitionSubType, DataPartitionSubType, PARTITION_TABLE_MAX_LEN, PartitionType,
};
use esp_storage::FlashStorage;
use sha2::{Digest, Sha256};

use crate::espnow_packet::{self, MAX_PACKET_LEN, OTA_CHUNK_LEN, OTA_INFO_OFFSET, OtaInfo, Packet};
use crate::hw::Radio;
use crate::log_line;

/// Wait for the gateway's answer to one request (it answers in a few ms).
const REPLY_WAIT_MS: u64 = 120;
/// Requests per chunk before giving up.
const ATTEMPTS: u32 = 8;
const SECTOR: usize = 4096;
/// On battery, below this an update is refused: a brown-out while writing
/// only costs the attempt, but the cell should be charged anyway.
const MIN_BATTERY_MV: u16 = 3700;
const IMAGE_MAGIC: u8 = 0xE9;
const ESP32C6_CHIP_ID: u16 = 13;
/// `esp_app_desc_t` follows the image header and first segment header.
const APP_DESC_OFFSET: usize = 32;
const APP_DESC_MAGIC: u32 = 0xABCD_5432;
const APP_DESC_PROJECT_NAME: usize = APP_DESC_OFFSET + 48;

/// The partition table has two OTA slots and otadata (checked at boot).
/// Readings carry `FLAG_OTA` only then.
pub static CAPABLE: AtomicBool = AtomicBool::new(false);

struct Slots {
    booted: AppPartitionSubType,
    target: AppPartitionSubType,
    target_offset: u32,
    target_len: u32,
}

/// The slot we run from and the other one, which an update is written to.
fn find_slots(flash: &mut FlashStorage<'static>) -> Result<Slots, &'static str> {
    let mut buf = [0u8; PARTITION_TABLE_MAX_LEN];
    let pt = partitions::read_partition_table(flash, &mut buf).map_err(|_| "partition table unreadable")?;
    if !pt
        .iter()
        .any(|p| p.partition_type() == PartitionType::Data(DataPartitionSubType::Ota))
    {
        return Err("no otadata partition (flash with the OTA partitions.csv)");
    }
    let booted = pt.booted_partition().ok().flatten().ok_or("booted partition unknown")?;
    let PartitionType::App(booted_sub) = booted.partition_type() else {
        return Err("booted partition is not an app");
    };
    let is_ota = |t: PartitionType| matches!(t, PartitionType::App(s) if s == AppPartitionSubType::Ota0 || s == AppPartitionSubType::Ota1);
    if !is_ota(booted.partition_type()) {
        return Err("not running from an OTA slot");
    }
    let target = pt
        .iter()
        .find(|p| is_ota(p.partition_type()) && p.offset() != booted.offset())
        .ok_or("no second OTA slot")?;
    let PartitionType::App(target_sub) = target.partition_type() else {
        return Err("no second OTA slot");
    };
    Ok(Slots {
        booted: booted_sub,
        target: target_sub,
        target_offset: target.offset(),
        target_len: target.len(),
    })
}

/// Checks the partition layout once per boot and sets [`CAPABLE`].
pub fn check_capable(flash: &mut FlashStorage<'static>) {
    let ok = match find_slots(flash) {
        Ok(_) => true,
        Err(e) => {
            log_line!("OTA: not available: {}", e);
            false
        }
    };
    CAPABLE.store(ok, Ordering::Relaxed);
}

/// Selects `target` in otadata; the bootloader starts it on the next reset.
fn activate(flash: &mut FlashStorage<'static>, slots: &Slots) -> Result<(), &'static str> {
    let mut buf = [0u8; PARTITION_TABLE_MAX_LEN];
    let pt = partitions::read_partition_table(flash, &mut buf).map_err(|_| "partition table unreadable")?;
    let otadata = pt
        .find_partition(PartitionType::Data(DataPartitionSubType::Ota))
        .ok()
        .flatten()
        .ok_or("no otadata partition")?;
    let mut ota = Ota::new(otadata.as_embedded_storage(flash), 2).map_err(|_| "otadata invalid")?;
    // Erased otadata reads as "factory"; record the running slot first so the
    // sequence numbers step from it (as the bootloader expects).
    if ota.current_app_partition().map_err(|_| "otadata unreadable")? != slots.booted {
        ota.set_current_app_partition(slots.booted).map_err(|_| "otadata write failed")?;
    }
    ota.set_current_app_partition(slots.target).map_err(|_| "otadata write failed")?;
    if ota.current_app_partition().map_err(|_| "otadata unreadable")? != slots.target {
        return Err("otadata did not switch");
    }
    Ok(())
}

/// Rejects images that are not an ESP32-C6 app of this project.
fn check_image_start(bytes: &[u8]) -> Result<(), &'static str> {
    if bytes.len() < APP_DESC_PROJECT_NAME + 32 {
        return Err("first chunk too short");
    }
    if bytes[0] != IMAGE_MAGIC {
        return Err("not an ESP app image");
    }
    if u16::from_le_bytes([bytes[12], bytes[13]]) != ESP32C6_CHIP_ID {
        return Err("image is not for ESP32-C6");
    }
    let magic = u32::from_le_bytes(bytes[APP_DESC_OFFSET..APP_DESC_OFFSET + 4].try_into().unwrap());
    if magic != APP_DESC_MAGIC {
        return Err("image has no app descriptor");
    }
    let name = &bytes[APP_DESC_PROJECT_NAME..APP_DESC_PROJECT_NAME + 32];
    let name = &name[..name.iter().position(|b| *b == 0).unwrap_or(name.len())];
    if name != env!("CARGO_PKG_NAME").as_bytes() {
        return Err("image is for another project");
    }
    Ok(())
}

/// Sends one OtaRequest and returns the matching reply, retrying on silence.
async fn request(radio: &mut Radio, key: &[u8], gateway: &[u8; 6], offset: u32) -> Option<Packet> {
    let mut buf = [0u8; MAX_PACKET_LEN];
    let len = espnow_packet::encode_ota_request(key, &radio.own_mac, offset, &mut buf);
    for _ in 0..ATTEMPTS {
        if radio.esp_now.send_async(gateway, &buf[..len]).await.is_err() {
            Timer::after(Duration::from_millis(10)).await;
            continue;
        }
        let deadline = Instant::now() + Duration::from_millis(REPLY_WAIT_MS);
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            let Ok(rx) = with_timeout(remaining, radio.esp_now.receive_async()).await else {
                break;
            };
            if rx.info.src_address != *gateway {
                continue;
            }
            match espnow_packet::decode(key, gateway, rx.data()) {
                Ok(p @ Packet::OtaInfo(_)) if offset == OTA_INFO_OFFSET => return Some(p),
                Ok(p @ Packet::OtaData(d)) if d.offset == offset => return Some(p),
                _ => {}
            }
        }
    }
    None
}

fn write_sector(flash: &mut FlashStorage<'static>, addr: u32, sector: &[u8]) -> Result<(), &'static str> {
    flash.erase(addr, addr + SECTOR as u32).map_err(|_| "flash erase failed")?;
    flash.write(addr, sector).map_err(|_| "flash write failed")
}

/// SHA-256 of `len` bytes at `addr`, read back from flash.
fn flash_sha256(flash: &mut FlashStorage<'static>, addr: u32, len: u32, buf: &mut [u8]) -> Result<[u8; 32], &'static str> {
    let mut hasher = Sha256::new();
    let mut done = 0u32;
    while done < len {
        flash.read(addr + done, buf).map_err(|_| "flash read failed")?;
        let n = (len - done).min(buf.len() as u32) as usize;
        hasher.update(&buf[..n]);
        done += n as u32;
    }
    Ok(hasher.finalize().into())
}

/// Downloads and installs the update the gateway offers, then restarts.
/// Returns only on failure; the running firmware stays selected then.
pub async fn run(
    radio: &mut Radio,
    key: &[u8],
    gateway: &[u8; 6],
    flash: &mut FlashStorage<'static>,
    battery_mv: u16,
    usb_power: bool,
) -> Result<(), &'static str> {
    if !usb_power && battery_mv != 0 && battery_mv < MIN_BATTERY_MV {
        return Err("battery too low for an update (charge it first)");
    }
    let slots = find_slots(flash)?;
    let Some(Packet::OtaInfo(info)) = request(radio, key, gateway, OTA_INFO_OFFSET).await else {
        return Err("gateway did not send the update info");
    };
    let info: OtaInfo = info;
    log_line!(
        "OTA: {} -> {}, {} bytes, writing to {:?} at 0x{:x}",
        crate::FW_VERSION,
        info.version(),
        info.size,
        slots.target,
        slots.target_offset
    );
    if info.version() == crate::FW_VERSION {
        return Err("already running this version");
    }
    if info.size == 0 || info.size > slots.target_len {
        return Err("image does not fit the OTA slot");
    }

    let started = Instant::now();
    let mut hasher = Sha256::new();
    let mut sector = alloc::vec![0xFFu8; SECTOR];
    let mut filled = 0usize;
    let mut sector_addr = slots.target_offset;
    let mut offset = 0u32;
    let mut next_log = 64 * 1024;
    while offset < info.size {
        let Some(Packet::OtaData(chunk)) = request(radio, key, gateway, offset).await else {
            return Err("gateway stopped sending the image");
        };
        let bytes = chunk.bytes();
        let expected = (info.size - offset).min(OTA_CHUNK_LEN as u32) as usize;
        if bytes.len() != expected {
            return Err("unexpected chunk length");
        }
        if offset == 0 {
            check_image_start(bytes)?;
        }
        hasher.update(bytes);
        let mut rest = bytes;
        while !rest.is_empty() {
            let n = rest.len().min(SECTOR - filled);
            sector[filled..filled + n].copy_from_slice(&rest[..n]);
            filled += n;
            rest = &rest[n..];
            if filled == SECTOR {
                write_sector(flash, sector_addr, &sector)?;
                sector_addr += SECTOR as u32;
                filled = 0;
                sector.fill(0xFF);
            }
        }
        offset += bytes.len() as u32;
        if offset >= next_log {
            log_line!("OTA: {} / {} bytes", offset, info.size);
            next_log += 64 * 1024;
        }
    }
    if filled > 0 {
        write_sector(flash, sector_addr, &sector)?;
    }
    let received: [u8; 32] = hasher.finalize().into();
    if received != info.sha256 {
        return Err("image SHA-256 does not match");
    }
    if flash_sha256(flash, slots.target_offset, info.size, &mut sector)? != info.sha256 {
        return Err("image in flash does not match (write error)");
    }
    activate(flash, &slots)?;
    log_line!(
        "OTA: {} installed in {} s, restarting",
        info.version(),
        started.elapsed().as_secs()
    );
    Timer::after(Duration::from_millis(100)).await;
    esp_hal::system::software_reset()
}
