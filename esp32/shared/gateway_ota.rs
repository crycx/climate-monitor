//! Firmware updates for ESP-NOW sensors, gateway side (C3 and C6 gateways).
//!
//! 1. `esp32/tools/publish_ota.py` publishes a sensor image retained to
//!    `<base>/ota/xiao-c6-sensor/image` and its description (version, size,
//!    SHA-256) to `.../meta`.
//! 2. Every MQTT session for a sensor reads `meta` ([`set_latest`]); HA's
//!    `update` entity shows installed vs. latest version.
//! 3. "Install" in HA leaves `install` on `<base>/<device>/set/ota`. The
//!    gateway downloads the image into its own spare OTA slot ([`stage`],
//!    checked against the SHA-256) and arms the sensor ([`arm`]).
//! 4. The Config reply to the sensor's next reading sets `SET_OTA`
//!    ([`offer`]); the sensor then pulls the image chunk by chunk, answered
//!    from flash by [`serve`] in the ESP-NOW receive task.
//! 5. The sensor restarts into the new firmware; its next reading reports
//!    the new version, which ends the update. A sensor that comes back with
//!    the old version is offered it once more, then the update is marked
//!    failed.
//!
//! The staging slot is the gateway's own inactive OTA slot (the gateway does
//! not update itself, so it is free).

use alloc::{format, string::String, vec, vec::Vec};
use core::cell::RefCell;

use embassy_net::{IpEndpoint, Stack, tcp::TcpSocket};
use embassy_time::{Duration, Instant};
use embedded_storage::nor_flash::NorFlash;
use esp_bootloader_esp_idf::partitions::{self, AppPartitionSubType, PARTITION_TABLE_MAX_LEN, PartitionType};
use esp_storage::FlashStorage;
use sha2::{Digest, Sha256};

use crate::espnow_packet::{OTA_CHUNK_LEN, OTA_INFO_OFFSET, OtaInfo, Reading};
use crate::log_runtime_line;

/// Project whose images are published for the ESP-NOW sensors.
pub const PROJECT: &str = "xiao-c6-sensor";
/// Times an armed sensor is offered the update before it counts as failed.
const MAX_ATTEMPTS: u8 = 2;
const MAX_SENSORS: usize = 16;
const SECTOR: usize = 4096;
/// After SUBACK, how long to wait for the retained image to start.
const IMAGE_WAIT_SECS: u64 = 10;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Offered with the next reading.
    Armed,
    /// The sensor is requesting chunks.
    Running,
    Failed,
}

#[derive(Clone, Copy)]
struct SensorOta {
    mac: [u8; 6],
    phase: Phase,
    attempts: u8,
}

struct Shared {
    flash: Option<FlashStorage<'static>>,
    /// Spare OTA slot: (flash offset, length).
    staging: Option<(u32, u32)>,
    /// Last image description seen on the broker.
    latest: Option<OtaInfo>,
    /// Image held (and verified) in the staging slot.
    staged: Option<OtaInfo>,
    sensors: [Option<SensorOta>; MAX_SENSORS],
}

static SHARED: critical_section::Mutex<RefCell<Shared>> = critical_section::Mutex::new(RefCell::new(Shared {
    flash: None,
    staging: None,
    latest: None,
    staged: None,
    sensors: [None; MAX_SENSORS],
}));

fn with_shared<R>(f: impl FnOnce(&mut Shared) -> R) -> R {
    critical_section::with(|cs| f(&mut SHARED.borrow_ref_mut(cs)))
}

/// Runs `f` on the flash outside the critical section (erases take tens of
/// ms). Tasks share one executor and `f` does not await, so it cannot race.
fn with_flash<R>(f: impl FnOnce(&mut FlashStorage<'static>) -> R) -> Option<R> {
    let mut flash = with_shared(|s| s.flash.take())?;
    let result = f(&mut flash);
    with_shared(|s| s.flash = Some(flash));
    Some(result)
}

/// Finds the spare OTA slot. Without one (old partition table) sensor
/// updates are disabled.
pub fn init(mut flash: FlashStorage<'static>) {
    let mut buf = [0u8; PARTITION_TABLE_MAX_LEN];
    let staging = partitions::read_partition_table(&mut flash, &mut buf).ok().and_then(|pt| {
        let booted = pt.booted_partition().ok().flatten()?;
        pt.iter()
            .find(|p| {
                matches!(p.partition_type(), PartitionType::App(s) if s == AppPartitionSubType::Ota0 || s == AppPartitionSubType::Ota1)
                    && p.offset() != booted.offset()
            })
            .map(|p| (p.offset(), p.len()))
    });
    match staging {
        Some((offset, len)) => log_runtime_line(
            format!("OTA: sensor images are staged at 0x{:x} ({} KB)", offset, len / 1024).as_str(),
        ),
        None => log_runtime_line("OTA: no spare OTA slot (old partitions.csv), sensor updates disabled"),
    }
    with_shared(|s| {
        s.flash = Some(flash);
        s.staging = staging;
    });
}

pub fn meta_topic(base: &str) -> String {
    format!("{}/ota/{}/meta", base, PROJECT)
}

fn image_topic(base: &str) -> String {
    format!("{}/ota/{}/image", base, PROJECT)
}

fn json_field<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let at = json.find(format!("\"{}\"", key).as_str())? + key.len() + 2;
    let rest = json[at..].trim_start().strip_prefix(':')?.trim_start();
    if let Some(quoted) = rest.strip_prefix('"') {
        quoted.split('"').next()
    } else {
        rest.split(|c: char| c == ',' || c == '}').next().map(str::trim)
    }
}

/// `{"version":"…","size":…,"sha256":"<64 hex>"}` as published by the tool.
pub fn parse_meta(payload: &[u8]) -> Option<OtaInfo> {
    let json = core::str::from_utf8(payload).ok()?;
    let version = json_field(json, "version")?;
    let size: u32 = json_field(json, "size")?.parse().ok()?;
    let hex = json_field(json, "sha256")?.as_bytes();
    if hex.len() != 64 || version.is_empty() {
        return None;
    }
    let mut sha256 = [0u8; 32];
    for (i, byte) in sha256.iter_mut().enumerate() {
        let pair = core::str::from_utf8(&hex[2 * i..2 * i + 2]).ok()?;
        *byte = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(OtaInfo::new(size, sha256, version))
}

/// The image on the broker, as read in the latest MQTT session (None: no
/// image published).
pub fn set_latest(info: Option<OtaInfo>) {
    with_shared(|s| s.latest = info);
}

fn latest() -> Option<OtaInfo> {
    with_shared(|s| s.latest)
}

pub fn in_progress(mac: &[u8; 6]) -> bool {
    with_shared(|s| {
        s.sensors
            .iter()
            .flatten()
            .any(|e| &e.mac == mac && e.phase != Phase::Failed)
    })
}

fn set_phase(mac: &[u8; 6], phase: Phase) {
    with_shared(|s| {
        let entry = SensorOta { mac: *mac, phase, attempts: 0 };
        if let Some(slot) = s.sensors.iter_mut().find(|e| e.is_some_and(|e| &e.mac == mac)) {
            *slot = Some(entry);
        } else if let Some(slot) = s.sensors.iter_mut().find(|e| e.is_none()) {
            *slot = Some(entry);
        } else {
            s.sensors[0] = Some(entry);
        }
    });
}

/// State JSON for the sensor's HA `update` entity.
pub fn ha_state(mac: &[u8; 6], installed: &str) -> String {
    let latest = latest().map(|i| String::from(i.version())).unwrap_or_else(|| String::from(installed));
    format!(
        r#"{{"installed_version":"{}","latest_version":"{}","in_progress":{}}}"#,
        crate::json_escape(installed),
        crate::json_escape(latest.as_str()),
        in_progress(mac)
    )
}

/// Called for every reading before the Config reply: true = set `SET_OTA`.
/// Also ends an update once the sensor reports the staged version.
pub fn offer(mac: &[u8; 6], r: &Reading) -> bool {
    enum Outcome {
        No,
        Offer(u8),
        Done,
        GaveUp,
    }
    let outcome = with_shared(|s| {
        let staged = s.staged;
        let Some(slot) = s.sensors.iter_mut().find(|e| e.is_some_and(|e| &e.mac == mac)) else {
            return Outcome::No;
        };
        let entry = slot.as_mut().unwrap();
        if staged.is_some_and(|i| i.version() == r.version()) {
            *slot = None;
            return Outcome::Done;
        }
        if entry.phase == Phase::Failed {
            return Outcome::No;
        }
        if staged.is_none() || !r.ota_capable() || entry.attempts >= MAX_ATTEMPTS {
            entry.phase = Phase::Failed;
            return Outcome::GaveUp;
        }
        entry.attempts += 1;
        entry.phase = Phase::Armed;
        Outcome::Offer(entry.attempts)
    });
    let mac_hex = crate::espnow_gateway::mac_hex(mac);
    match outcome {
        Outcome::No => false,
        Outcome::Offer(n) => {
            log_runtime_line(format!("OTA {}: offering the update (attempt {})", mac_hex, n).as_str());
            true
        }
        Outcome::Done => {
            log_runtime_line(format!("OTA {}: now running {}", mac_hex, r.version()).as_str());
            false
        }
        Outcome::GaveUp => {
            log_runtime_line(format!("OTA {}: failed, still on {}", mac_hex, r.version()).as_str());
            false
        }
    }
}

/// Answer to an OtaRequest.
pub enum Reply {
    Info(OtaInfo),
    Data { offset: u32, len: usize, data: [u8; OTA_CHUNK_LEN] },
}

/// Serves an OtaRequest from `mac`; None if no update is armed for it.
pub fn serve(mac: &[u8; 6], offset: u32) -> Option<Reply> {
    let (info, staging) = with_shared(|s| {
        let entry = s.sensors.iter_mut().flatten().find(|e| &e.mac == mac)?;
        if entry.phase == Phase::Failed {
            return None;
        }
        entry.phase = Phase::Running;
        Some((s.staged?, s.staging?))
    })?;
    if offset == OTA_INFO_OFFSET {
        return Some(Reply::Info(info));
    }
    if offset >= info.size {
        return None;
    }
    let len = ((info.size - offset) as usize).min(OTA_CHUNK_LEN);
    let mut data = [0u8; OTA_CHUNK_LEN];
    let read = with_flash(|f| embedded_storage::ReadStorage::read(f, staging.0 + offset, &mut data[..len]))?;
    read.ok()?;
    if offset == 0 {
        log_runtime_line(format!("OTA {}: sending {} bytes", crate::espnow_gateway::mac_hex(mac), info.size).as_str());
    }
    Some(Reply::Data { offset, len, data })
}

async fn read_exact(socket: &mut TcpSocket<'_>, buf: &mut [u8], timeout: Duration) -> Option<()> {
    let mut off = 0;
    while off < buf.len() {
        let n = embassy_time::with_timeout(timeout, socket.read(&mut buf[off..])).await.ok()?.ok()?;
        if n == 0 {
            return None;
        }
        off += n;
    }
    Some(())
}

/// Fixed header of the next MQTT packet: (first byte, remaining length).
async fn read_header(socket: &mut TcpSocket<'_>, first_wait: Duration) -> Option<(u8, usize)> {
    let io = Duration::from_secs(crate::MQTT_IO_TIMEOUT_SECS);
    let mut b = [0u8; 1];
    read_exact(socket, &mut b, first_wait).await?;
    let kind = b[0];
    let (mut len, mut shift) = (0usize, 0);
    loop {
        read_exact(socket, &mut b, io).await?;
        len |= ((b[0] & 0x7f) as usize) << shift;
        if b[0] & 0x80 == 0 {
            return Some((kind, len));
        }
        shift += 7;
        if shift > 21 {
            return None;
        }
    }
}

async fn skip(socket: &mut TcpSocket<'_>, mut len: usize, buf: &mut [u8]) -> Option<()> {
    let io = Duration::from_secs(crate::MQTT_IO_TIMEOUT_SECS);
    while len > 0 {
        let n = len.min(buf.len());
        read_exact(socket, &mut buf[..n], io).await?;
        len -= n;
    }
    Some(())
}

/// Streams the retained image into the staging slot, checking size and hash.
async fn receive_image(
    socket: &mut TcpSocket<'_>,
    topic: &str,
    info: &OtaInfo,
    slot: (u32, u32),
) -> Result<(), &'static str> {
    let io = Duration::from_secs(crate::MQTT_IO_TIMEOUT_SECS);
    let mut sector: Vec<u8> = vec![0xFF; SECTOR];
    let mut subscribed = false;
    loop {
        let wait = Duration::from_secs(if subscribed { IMAGE_WAIT_SECS } else { crate::MQTT_IO_TIMEOUT_SECS });
        let (kind, len) = read_header(socket, wait).await.ok_or(if subscribed {
            "image not on the broker (run tools/publish_ota.py)"
        } else {
            "broker did not answer the subscribe"
        })?;
        if kind >> 4 == 9 {
            subscribed = true;
            skip(socket, len, &mut sector).await.ok_or("read failed")?;
            continue;
        }
        if kind >> 4 != 3 || len < 2 {
            skip(socket, len, &mut sector).await.ok_or("read failed")?;
            continue;
        }
        let mut tl = [0u8; 2];
        read_exact(socket, &mut tl, io).await.ok_or("read failed")?;
        let topic_len = u16::from_be_bytes(tl) as usize;
        let qos_extra = if kind & 0x06 != 0 { 2 } else { 0 };
        if topic_len + 2 + qos_extra > len || topic_len > SECTOR {
            return Err("bad PUBLISH");
        }
        read_exact(socket, &mut sector[..topic_len], io).await.ok_or("read failed")?;
        let matches = &sector[..topic_len] == topic.as_bytes();
        skip(socket, qos_extra, &mut tl).await.ok_or("read failed")?;
        let payload_len = len - 2 - topic_len - qos_extra;
        if !matches {
            skip(socket, payload_len, &mut sector).await.ok_or("read failed")?;
            continue;
        }
        if payload_len != info.size as usize {
            return Err("image size differs from meta (publish again)");
        }
        if payload_len > slot.1 as usize {
            return Err("image does not fit the staging slot");
        }

        let mut hasher = Sha256::new();
        let mut done = 0usize;
        while done < payload_len {
            let n = (payload_len - done).min(SECTOR);
            sector.fill(0xFF);
            read_exact(socket, &mut sector[..n], io).await.ok_or("download interrupted")?;
            hasher.update(&sector[..n]);
            let addr = slot.0 + done as u32;
            with_flash(|f| {
                NorFlash::erase(f, addr, addr + SECTOR as u32)?;
                NorFlash::write(f, addr, &sector)
            })
            .ok_or("flash busy")?
            .map_err(|_| "flash write failed")?;
            done += n;
        }
        let digest: [u8; 32] = hasher.finalize().into();
        return if digest == info.sha256 { Ok(()) } else { Err("image SHA-256 differs from meta") };
    }
}

/// Makes sure the staging slot holds the image the broker describes.
async fn stage(stack: Stack<'static>, s: &Broker<'_>, info: &OtaInfo) -> bool {
    if with_shared(|sh| sh.staged == Some(*info)) {
        return true;
    }
    let Some(slot) = with_shared(|sh| sh.staging) else {
        log_runtime_line("OTA: no spare OTA slot on this gateway (flash it with the OTA partitions.csv)");
        return false;
    };
    let Some(ip) = crate::resolve_host_to_ip(stack, s.host).await else {
        log_runtime_line("OTA: broker DNS/host resolution failed");
        return false;
    };
    with_shared(|sh| sh.staged = None);
    log_runtime_line(format!("OTA: downloading {} ({} bytes)", info.version(), info.size).as_str());
    let started = Instant::now();

    let mut rx_buf = vec![0u8; 4096];
    let mut tx_buf = vec![0u8; 512];
    let mut socket = TcpSocket::new(stack, &mut rx_buf, &mut tx_buf);
    let connected = embassy_time::with_timeout(
        Duration::from_secs(crate::MQTT_CONNECT_TIMEOUT_SECS),
        socket.connect(IpEndpoint::new(ip, s.port)),
    )
    .await;
    let io = Duration::from_secs(crate::MQTT_IO_TIMEOUT_SECS);
    let client_id = format!("{}-ota-{}", s.base, Instant::now().as_ticks() % 1_000_000);
    let session = if matches!(connected, Ok(Ok(()))) {
        embassy_time::with_timeout(io, crate::mqtt_connect(&mut socket, client_id.as_str(), s.username, s.password))
            .await
            .is_ok_and(|r| r.is_ok())
    } else {
        false
    };
    if !session {
        log_runtime_line("OTA: broker connect failed");
        socket.abort();
        return false;
    }

    let topic = image_topic(s.base);
    let mut packet: Vec<u8> = Vec::with_capacity(topic.len() + 8);
    packet.push(0x82); // SUBSCRIBE
    crate::encode_remaining_length(&mut packet, 2 + 2 + topic.len() + 1);
    packet.extend_from_slice(&[0x00, 0x02]); // packet id 2
    packet.extend_from_slice(&(topic.len() as u16).to_be_bytes());
    packet.extend_from_slice(topic.as_bytes());
    packet.push(0x00); // QoS 0
    let result = match crate::socket_write_all(&mut socket, &packet).await {
        Ok(()) => receive_image(&mut socket, topic.as_str(), info, slot).await,
        Err(_) => Err("subscribe failed"),
    };
    let _ = embassy_time::with_timeout(io, crate::mqtt_disconnect(&mut socket)).await;
    socket.close();
    let _ = embassy_time::with_timeout(io, socket.flush()).await;

    match result {
        Ok(()) => {
            with_shared(|sh| sh.staged = Some(*info));
            log_runtime_line(
                format!("OTA: {} staged and verified in {} s", info.version(), started.elapsed().as_secs()).as_str(),
            );
            true
        }
        Err(e) => {
            log_runtime_line(format!("OTA: download failed: {}", e).as_str());
            false
        }
    }
}

/// MQTT broker settings for the download session.
pub struct Broker<'a> {
    pub host: &'a str,
    pub port: u16,
    pub username: &'a str,
    pub password: &'a str,
    pub base: &'a str,
}

/// "Install" from HA for the sensor `mac` that runs `installed`: stages the
/// published image and arms the sensor.
pub async fn install(stack: Stack<'static>, broker: &Broker<'_>, mac: &[u8; 6], installed: &str) {
    let mac_hex = crate::espnow_gateway::mac_hex(mac);
    let Some(info) = latest() else {
        log_runtime_line(format!("OTA {}: no firmware published (run tools/publish_ota.py)", mac_hex).as_str());
        set_phase(mac, Phase::Failed);
        return;
    };
    if info.version() == installed {
        log_runtime_line(format!("OTA {}: already running {}", mac_hex, installed).as_str());
        return;
    }
    if in_progress(mac) {
        return;
    }
    if !stage(stack, broker, &info).await {
        set_phase(mac, Phase::Failed);
        return;
    }
    set_phase(mac, Phase::Armed);
    log_runtime_line(
        format!("OTA {}: {} -> {} armed, starts after its next reading", mac_hex, installed, info.version()).as_str(),
    );
}
