//! ESP-NOW wire format shared by the battery sensor node (`xiao-c6-sensor`)
//! and the mains-powered gateway (`c6`).
//!
//! Every packet ends with a truncated HMAC-SHA256 tag computed over the
//! sender MAC address followed by the packet body. The gateway only accepts
//! readings whose tag matches the shared `ESPNOW_KEY`, and the sensor only
//! trusts acknowledgements that carry a valid tag from the gateway.
//!
//! Layout (little endian):
//!
//! ```text
//! Reading / Discover
//!   0..2    magic "CM"
//!   2       version
//!   3       kind (1 = reading, 2 = reading + ack request, 3 = ack)
//!   4..8    boot_id      u32  random per sensor cold boot
//!   8..12   seq          u32  increments on every transmission
//!   12..14  temp_centi   i16  temperature * 100 (°C)
//!   14..16  hum_centi    u16  relative humidity * 100 (%)
//!   16..18  battery_mv   u16  0 = unknown
//!   18..20  interval_s   u16  sensor's sleep interval
//!   20      flags        u8   bit0 = sensor read failed,
//!                               bit1 = on USB power (battery charging),
//!                               bit2 = config trailer present (FLAG_CONFIG),
//!                               bit3 = version trailer present (FLAG_VERSION),
//!                               bit4 = sensor can update over the air (FLAG_OTA)
//!   21      name_len     u8   0..=MAX_NAME_LEN
//!   22..    name         UTF-8, name_len bytes
//!   ..+2    temp_offset  i16  degC * 100 } only with FLAG_CONFIG: the
//!   ..+2    hum_offset   i16  % * 100    } sensor's current offsets
//!   ..+1    version_len  u8   } only with FLAG_VERSION: firmware version
//!   ..      version      UTF-8, version_len bytes (<= MAX_VERSION_LEN)
//!   ..+8    tag
//!
//! Ack
//!   0..4    magic, version, kind = 3
//!   4..8    boot_id      echoed from the reading
//!   8..12   seq          echoed from the reading
//!   12      channel      gateway WiFi channel
//!   13..21  tag
//!
//! Config (gateway -> sensor, reply to every reading)
//!   0..4    magic, version, kind = 4
//!   4..8    boot_id      echoed from the reading
//!   8..12   seq          echoed from the reading
//!   12      set          u8   bit0 temp_offset, bit1 hum_offset, bit2 interval,
//!                               bit3 firmware update waiting (SET_OTA)
//!   13..15  temp_offset  i16  degC * 100
//!   15..17  hum_offset   i16  % * 100
//!   17..19  interval_s   u16
//!   19      caps         u8   gateway capabilities (CAP_*); missing in
//!                             gateways built before it (19-byte body)
//!   ..+8    tag
//!
//! OtaRequest (sensor -> gateway, after a Config with SET_OTA)
//!   0..4    magic, version, kind = 5
//!   4..8    offset       u32  image offset to send, OTA_INFO_OFFSET = info
//!   8..16   tag
//!
//! OtaInfo (gateway -> sensor, reply to offset OTA_INFO_OFFSET)
//!   0..4    magic, version, kind = 6
//!   4..8    size         u32  image size in bytes
//!   8..40   sha256       image SHA-256
//!   40      version_len  u8
//!   41..57  version      UTF-8, zero padded
//!   57..65  tag
//!
//! OtaData (gateway -> sensor)
//!   0..4    magic, version, kind = 7
//!   4..8    offset       u32
//!   8       len          u8   1..=OTA_CHUNK_LEN
//!   9..     data
//!   ..+8    tag
//! ```
//!
//! Firmware update (see `shared/gateway_ota.rs` and the sensor's `ota.rs`):
//! the sensor pulls the image chunk by chunk (stop and wait), so the gateway
//! only answers requests. Every packet carries the HMAC tag and the sensor
//! checks the image SHA-256 before switching to it.
//!
//! Remote config: the gateway answers each reading with a Config packet,
//! with `set = 0` when nothing is pending. A sensor sets FLAG_CONFIG (and
//! appends its offsets) only after its current gateway has answered, so
//! gateways that predate Config, whose decoder checks the exact length,
//! keep accepting its readings. Likewise a sensor adds the version trailer
//! only when its gateway's Config reply has CAP_VERSION.

#![allow(dead_code)]

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

pub const MAGIC: [u8; 2] = *b"CM";
pub const VERSION: u8 = 1;
pub const TAG_LEN: usize = 8;
pub const MAX_NAME_LEN: usize = 24;
pub const MAX_VERSION_LEN: usize = 16;
const READING_HEADER_LEN: usize = 22;
const CONFIG_TRAILER_LEN: usize = 4;
const ACK_BODY_LEN: usize = 13;
/// Config body without the caps byte (gateways built before it).
const CONFIG_BODY_LEN_NO_CAPS: usize = 19;
const CONFIG_BODY_LEN: usize = 20;
const OTA_REQUEST_BODY_LEN: usize = 8;
const OTA_INFO_BODY_LEN: usize = 41 + MAX_VERSION_LEN;
const OTA_DATA_HEADER_LEN: usize = 9;
/// Image bytes per OtaData packet (ESP-NOW payloads are at most 250 bytes).
pub const OTA_CHUNK_LEN: usize = 200;
/// OtaRequest offset that asks for the OtaInfo.
pub const OTA_INFO_OFFSET: u32 = u32::MAX;
const MAX_READING_LEN: usize =
    READING_HEADER_LEN + MAX_NAME_LEN + CONFIG_TRAILER_LEN + 1 + MAX_VERSION_LEN + TAG_LEN;
const MAX_OTA_DATA_LEN: usize = OTA_DATA_HEADER_LEN + OTA_CHUNK_LEN + TAG_LEN;
pub const MAX_PACKET_LEN: usize =
    if MAX_READING_LEN > MAX_OTA_DATA_LEN { MAX_READING_LEN } else { MAX_OTA_DATA_LEN };

pub const FLAG_SENSOR_ERROR: u8 = 0x01;
/// Sensor runs on USB power: the charger holds the cell at ~4.2 V, so
/// `battery_mv` is the charge voltage, not the state of charge. Gateways that
/// predate this flag ignore it.
pub const FLAG_USB_POWER: u8 = 0x02;
/// The reading carries the sensor's offsets (config trailer) and the sensor
/// applies [`ConfigMsg`] replies.
pub const FLAG_CONFIG: u8 = 0x04;
/// The reading carries the sensor's firmware version (version trailer).
pub const FLAG_VERSION: u8 = 0x08;

/// The sensor has OTA app slots and handles [`SET_OTA`].
pub const FLAG_OTA: u8 = 0x10;

/// [`ConfigMsg::caps`] bits: what the gateway accepts from the sensor.
pub const CAP_VERSION: u8 = 0x01;

/// [`ConfigMsg::set`] bits.
pub const SET_TEMP_OFFSET: u8 = 0x01;
pub const SET_HUM_OFFSET: u8 = 0x02;
pub const SET_INTERVAL: u8 = 0x04;
/// A firmware update is waiting: stay awake and send OtaRequests.
pub const SET_OTA: u8 = 0x08;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    /// Plain reading, delivered by unicast to a known gateway.
    Reading = 1,
    /// Reading broadcast while searching for the gateway; asks for an [`Ack`].
    Discover = 2,
    /// Gateway reply to a [`Kind::Discover`] packet.
    Ack = 3,
    /// Gateway reply to every reading: pending remote config (may be empty).
    Config = 4,
    OtaRequest = 5,
    OtaInfo = 6,
    OtaData = 7,
}

#[derive(Clone, Copy, Debug)]
pub struct Reading {
    pub boot_id: u32,
    pub seq: u32,
    pub temp_centi: i16,
    pub hum_centi: u16,
    pub battery_mv: u16,
    pub interval_s: u16,
    pub flags: u8,
    /// Only meaningful with [`FLAG_CONFIG`].
    pub temp_offset_centi: i16,
    /// Only meaningful with [`FLAG_CONFIG`].
    pub hum_offset_centi: i16,
    name: [u8; MAX_NAME_LEN],
    name_len: u8,
    /// Only meaningful with [`FLAG_VERSION`].
    version: [u8; MAX_VERSION_LEN],
    version_len: u8,
}

impl Reading {
    pub const fn empty() -> Self {
        Self {
            boot_id: 0,
            seq: 0,
            temp_centi: 0,
            hum_centi: 0,
            battery_mv: 0,
            interval_s: 0,
            flags: 0,
            temp_offset_centi: 0,
            hum_offset_centi: 0,
            name: [0; MAX_NAME_LEN],
            name_len: 0,
            version: [0; MAX_VERSION_LEN],
            version_len: 0,
        }
    }

    /// Stores `name`, truncated to [`MAX_NAME_LEN`] bytes on a char boundary.
    pub fn set_name(&mut self, name: &str) {
        self.name_len = copy_truncated(&mut self.name, name);
    }

    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len as usize]).unwrap_or("")
    }

    /// Stores the firmware version (truncated to [`MAX_VERSION_LEN`] bytes);
    /// it is sent only with [`FLAG_VERSION`].
    pub fn set_version(&mut self, version: &str) {
        self.version_len = copy_truncated(&mut self.version, version);
    }

    /// Firmware version, empty if the reading has none.
    pub fn version(&self) -> &str {
        if self.flags & FLAG_VERSION == 0 {
            return "";
        }
        core::str::from_utf8(&self.version[..self.version_len as usize]).unwrap_or("")
    }

    pub fn sensor_ok(&self) -> bool {
        self.flags & FLAG_SENSOR_ERROR == 0
    }

    pub fn has_config(&self) -> bool {
        self.flags & FLAG_CONFIG != 0
    }

    pub fn usb_power(&self) -> bool {
        self.flags & FLAG_USB_POWER != 0
    }

    pub fn ota_capable(&self) -> bool {
        self.flags & FLAG_OTA != 0
    }

    pub fn temperature_c(&self) -> f32 {
        self.temp_centi as f32 / 100.0
    }

    pub fn humidity_pct(&self) -> f32 {
        self.hum_centi as f32 / 100.0
    }
}

/// Copies `s` into `buf`, truncated on a char boundary; returns the length.
fn copy_truncated(buf: &mut [u8], s: &str) -> u8 {
    let mut end = s.len().min(buf.len());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    buf.fill(0);
    buf[..end].copy_from_slice(&s.as_bytes()[..end]);
    end as u8
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ack {
    pub boot_id: u32,
    pub seq: u32,
    pub channel: u8,
}

/// Remote config for one sensor, sent by the gateway as the reply to a
/// reading. Only fields whose bit is set in `set` are meant to be applied.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConfigMsg {
    pub boot_id: u32,
    pub seq: u32,
    pub set: u8,
    pub temp_offset_centi: i16,
    pub hum_offset_centi: i16,
    pub interval_s: u16,
    /// Gateway capabilities ([`CAP_VERSION`]); 0 from older gateways.
    pub caps: u8,
}

/// Image description sent before the data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OtaInfo {
    pub size: u32,
    pub sha256: [u8; 32],
    version: [u8; MAX_VERSION_LEN],
    version_len: u8,
}

impl OtaInfo {
    pub fn new(size: u32, sha256: [u8; 32], version: &str) -> Self {
        let mut info = Self {
            size,
            sha256,
            version: [0; MAX_VERSION_LEN],
            version_len: 0,
        };
        info.version_len = copy_truncated(&mut info.version, version);
        info
    }

    pub fn version(&self) -> &str {
        core::str::from_utf8(&self.version[..self.version_len as usize]).unwrap_or("")
    }
}

/// One chunk of the image.
#[derive(Clone, Copy, Debug)]
pub struct OtaData {
    pub offset: u32,
    pub len: u8,
    pub data: [u8; OTA_CHUNK_LEN],
}

impl OtaData {
    pub fn bytes(&self) -> &[u8] {
        &self.data[..self.len as usize]
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Packet {
    Reading { reading: Reading, wants_ack: bool },
    Ack(Ack),
    Config(ConfigMsg),
    OtaRequest { offset: u32 },
    OtaInfo(OtaInfo),
    OtaData(OtaData),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    TooShort,
    BadMagic,
    UnsupportedVersion,
    UnknownKind,
    BadLength,
    BadTag,
}

fn new_mac(key: &[u8], src_mac: &[u8; 6]) -> HmacSha256 {
    // HMAC accepts keys of any length, so this cannot fail.
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(src_mac);
    mac
}

fn append_tag(key: &[u8], src_mac: &[u8; 6], out: &mut [u8], body_len: usize) -> usize {
    let mut mac = new_mac(key, src_mac);
    mac.update(&out[..body_len]);
    let tag = mac.finalize().into_bytes();
    out[body_len..body_len + TAG_LEN].copy_from_slice(&tag[..TAG_LEN]);
    body_len + TAG_LEN
}

fn verify_tag(key: &[u8], src_mac: &[u8; 6], data: &[u8]) -> Result<(), DecodeError> {
    let body_len = data.len() - TAG_LEN;
    let mut mac = new_mac(key, src_mac);
    mac.update(&data[..body_len]);
    mac.verify_truncated_left(&data[body_len..])
        .map_err(|_| DecodeError::BadTag)
}

fn write_header(out: &mut [u8], kind: Kind) {
    out[0..2].copy_from_slice(&MAGIC);
    out[2] = VERSION;
    out[3] = kind as u8;
}

/// Encodes a reading. `kind` must be [`Kind::Reading`] or [`Kind::Discover`].
/// Returns the number of bytes written to `out`.
pub fn encode_reading(
    key: &[u8],
    src_mac: &[u8; 6],
    kind: Kind,
    r: &Reading,
    out: &mut [u8; MAX_PACKET_LEN],
) -> usize {
    debug_assert!(kind == Kind::Reading || kind == Kind::Discover);
    write_header(out, kind);
    out[4..8].copy_from_slice(&r.boot_id.to_le_bytes());
    out[8..12].copy_from_slice(&r.seq.to_le_bytes());
    out[12..14].copy_from_slice(&r.temp_centi.to_le_bytes());
    out[14..16].copy_from_slice(&r.hum_centi.to_le_bytes());
    out[16..18].copy_from_slice(&r.battery_mv.to_le_bytes());
    out[18..20].copy_from_slice(&r.interval_s.to_le_bytes());
    out[20] = r.flags;
    let name_len = r.name_len as usize;
    out[21] = r.name_len;
    out[READING_HEADER_LEN..READING_HEADER_LEN + name_len].copy_from_slice(&r.name[..name_len]);
    let mut body_len = READING_HEADER_LEN + name_len;
    if r.has_config() {
        out[body_len..body_len + 2].copy_from_slice(&r.temp_offset_centi.to_le_bytes());
        out[body_len + 2..body_len + 4].copy_from_slice(&r.hum_offset_centi.to_le_bytes());
        body_len += CONFIG_TRAILER_LEN;
    }
    if r.flags & FLAG_VERSION != 0 {
        let len = r.version_len as usize;
        out[body_len] = r.version_len;
        out[body_len + 1..body_len + 1 + len].copy_from_slice(&r.version[..len]);
        body_len += 1 + len;
    }
    append_tag(key, src_mac, out, body_len)
}

/// Encodes a remote-config reply. Returns the number of bytes written to `out`.
pub fn encode_config(
    key: &[u8],
    src_mac: &[u8; 6],
    msg: &ConfigMsg,
    out: &mut [u8; MAX_PACKET_LEN],
) -> usize {
    write_header(out, Kind::Config);
    out[4..8].copy_from_slice(&msg.boot_id.to_le_bytes());
    out[8..12].copy_from_slice(&msg.seq.to_le_bytes());
    out[12] = msg.set;
    out[13..15].copy_from_slice(&msg.temp_offset_centi.to_le_bytes());
    out[15..17].copy_from_slice(&msg.hum_offset_centi.to_le_bytes());
    out[17..19].copy_from_slice(&msg.interval_s.to_le_bytes());
    out[19] = msg.caps;
    append_tag(key, src_mac, out, CONFIG_BODY_LEN)
}

/// Encodes a firmware chunk request (`OTA_INFO_OFFSET` asks for the info).
pub fn encode_ota_request(key: &[u8], src_mac: &[u8; 6], offset: u32, out: &mut [u8; MAX_PACKET_LEN]) -> usize {
    write_header(out, Kind::OtaRequest);
    out[4..8].copy_from_slice(&offset.to_le_bytes());
    append_tag(key, src_mac, out, OTA_REQUEST_BODY_LEN)
}

pub fn encode_ota_info(key: &[u8], src_mac: &[u8; 6], info: &OtaInfo, out: &mut [u8; MAX_PACKET_LEN]) -> usize {
    write_header(out, Kind::OtaInfo);
    out[4..8].copy_from_slice(&info.size.to_le_bytes());
    out[8..40].copy_from_slice(&info.sha256);
    out[40] = info.version_len;
    out[41..41 + MAX_VERSION_LEN].copy_from_slice(&info.version);
    append_tag(key, src_mac, out, OTA_INFO_BODY_LEN)
}

/// Encodes a firmware chunk; `data` must be 1..=OTA_CHUNK_LEN bytes.
pub fn encode_ota_data(
    key: &[u8],
    src_mac: &[u8; 6],
    offset: u32,
    data: &[u8],
    out: &mut [u8; MAX_PACKET_LEN],
) -> usize {
    let len = data.len().min(OTA_CHUNK_LEN);
    write_header(out, Kind::OtaData);
    out[4..8].copy_from_slice(&offset.to_le_bytes());
    out[8] = len as u8;
    out[OTA_DATA_HEADER_LEN..OTA_DATA_HEADER_LEN + len].copy_from_slice(&data[..len]);
    append_tag(key, src_mac, out, OTA_DATA_HEADER_LEN + len)
}

/// Encodes an acknowledgement. Returns the number of bytes written to `out`.
pub fn encode_ack(
    key: &[u8],
    src_mac: &[u8; 6],
    ack: &Ack,
    out: &mut [u8; MAX_PACKET_LEN],
) -> usize {
    write_header(out, Kind::Ack);
    out[4..8].copy_from_slice(&ack.boot_id.to_le_bytes());
    out[8..12].copy_from_slice(&ack.seq.to_le_bytes());
    out[12] = ack.channel;
    append_tag(key, src_mac, out, ACK_BODY_LEN)
}

/// Parses and authenticates a packet received from `src_mac`.
pub fn decode(key: &[u8], src_mac: &[u8; 6], data: &[u8]) -> Result<Packet, DecodeError> {
    if data.len() < 4 + TAG_LEN {
        return Err(DecodeError::TooShort);
    }
    if data[0..2] != MAGIC {
        return Err(DecodeError::BadMagic);
    }
    if data[2] != VERSION {
        return Err(DecodeError::UnsupportedVersion);
    }

    let u32_at = |i: usize| u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
    let u16_at = |i: usize| u16::from_le_bytes([data[i], data[i + 1]]);

    match data[3] {
        k @ (1 | 2) => {
            if data.len() < READING_HEADER_LEN + TAG_LEN {
                return Err(DecodeError::TooShort);
            }
            let name_len = data[21] as usize;
            let flags = data[20];
            if name_len > MAX_NAME_LEN {
                return Err(DecodeError::BadLength);
            }
            let trailer = if flags & FLAG_CONFIG != 0 { CONFIG_TRAILER_LEN } else { 0 };
            let version_at = READING_HEADER_LEN + name_len + trailer;
            let mut body_len = version_at;
            let mut version_len = 0;
            if flags & FLAG_VERSION != 0 {
                if data.len() < version_at + 1 + TAG_LEN {
                    return Err(DecodeError::BadLength);
                }
                version_len = data[version_at] as usize;
                if version_len > MAX_VERSION_LEN {
                    return Err(DecodeError::BadLength);
                }
                body_len += 1 + version_len;
            }
            if data.len() != body_len + TAG_LEN {
                return Err(DecodeError::BadLength);
            }
            verify_tag(key, src_mac, data)?;

            let mut reading = Reading {
                boot_id: u32_at(4),
                seq: u32_at(8),
                temp_centi: u16_at(12) as i16,
                hum_centi: u16_at(14),
                battery_mv: u16_at(16),
                interval_s: u16_at(18),
                flags,
                temp_offset_centi: 0,
                hum_offset_centi: 0,
                name: [0; MAX_NAME_LEN],
                name_len: 0,
                version: [0; MAX_VERSION_LEN],
                version_len: 0,
            };
            if flags & FLAG_VERSION != 0 {
                let version = &data[version_at + 1..version_at + 1 + version_len];
                reading.set_version(core::str::from_utf8(version).unwrap_or(""));
            }
            if trailer != 0 {
                let at = READING_HEADER_LEN + name_len;
                reading.temp_offset_centi = u16_at(at) as i16;
                reading.hum_offset_centi = u16_at(at + 2) as i16;
            }
            let name = &data[READING_HEADER_LEN..READING_HEADER_LEN + name_len];
            // Invalid UTF-8 names are dropped rather than rejecting the reading.
            if let Ok(name) = core::str::from_utf8(name) {
                reading.set_name(name);
            }
            Ok(Packet::Reading {
                reading,
                wants_ack: k == Kind::Discover as u8,
            })
        }
        3 => {
            if data.len() != ACK_BODY_LEN + TAG_LEN {
                return Err(DecodeError::BadLength);
            }
            verify_tag(key, src_mac, data)?;
            Ok(Packet::Ack(Ack {
                boot_id: u32_at(4),
                seq: u32_at(8),
                channel: data[12],
            }))
        }
        4 => {
            let body_len = data.len() - TAG_LEN;
            if body_len != CONFIG_BODY_LEN && body_len != CONFIG_BODY_LEN_NO_CAPS {
                return Err(DecodeError::BadLength);
            }
            verify_tag(key, src_mac, data)?;
            Ok(Packet::Config(ConfigMsg {
                boot_id: u32_at(4),
                seq: u32_at(8),
                set: data[12],
                temp_offset_centi: u16_at(13) as i16,
                hum_offset_centi: u16_at(15) as i16,
                interval_s: u16_at(17),
                caps: if body_len == CONFIG_BODY_LEN { data[19] } else { 0 },
            }))
        }
        5 => {
            if data.len() != OTA_REQUEST_BODY_LEN + TAG_LEN {
                return Err(DecodeError::BadLength);
            }
            verify_tag(key, src_mac, data)?;
            Ok(Packet::OtaRequest { offset: u32_at(4) })
        }
        6 => {
            if data.len() != OTA_INFO_BODY_LEN + TAG_LEN {
                return Err(DecodeError::BadLength);
            }
            verify_tag(key, src_mac, data)?;
            let mut sha256 = [0u8; 32];
            sha256.copy_from_slice(&data[8..40]);
            let version_len = (data[40] as usize).min(MAX_VERSION_LEN);
            let version = core::str::from_utf8(&data[41..41 + version_len]).unwrap_or("");
            Ok(Packet::OtaInfo(OtaInfo::new(u32_at(4), sha256, version)))
        }
        7 => {
            if data.len() < OTA_DATA_HEADER_LEN + 1 + TAG_LEN {
                return Err(DecodeError::TooShort);
            }
            let len = data[8] as usize;
            if len == 0 || len > OTA_CHUNK_LEN || data.len() != OTA_DATA_HEADER_LEN + len + TAG_LEN {
                return Err(DecodeError::BadLength);
            }
            verify_tag(key, src_mac, data)?;
            let mut chunk = OtaData {
                offset: u32_at(4),
                len: len as u8,
                data: [0; OTA_CHUNK_LEN],
            };
            chunk.data[..len].copy_from_slice(&data[OTA_DATA_HEADER_LEN..OTA_DATA_HEADER_LEN + len]);
            Ok(Packet::OtaData(chunk))
        }
        _ => Err(DecodeError::UnknownKind),
    }
}

/// Rough LiPo state of charge from cell voltage under light load.
///
/// 100 % starts at 4.15 V, not 4.20 V: the charger stops at 4.20 V, but a
/// full cell rests at ~4.15-4.19 V once it is off the charger.
pub fn lipo_percent(battery_mv: u16) -> u8 {
    const CURVE: [(u16, u8); 11] = [
        (4150, 100),
        (4100, 90),
        (4000, 80),
        (3920, 70),
        (3870, 60),
        (3820, 50),
        (3790, 40),
        (3750, 30),
        (3700, 20),
        (3600, 10),
        (3300, 0),
    ];
    if battery_mv >= CURVE[0].0 {
        return 100;
    }
    for pair in CURVE.windows(2) {
        let (hi_mv, hi_pct) = pair[0];
        let (lo_mv, lo_pct) = pair[1];
        if battery_mv >= lo_mv {
            let span_mv = (hi_mv - lo_mv) as u32;
            let span_pct = (hi_pct - lo_pct) as u32;
            let above = (battery_mv - lo_mv) as u32;
            return lo_pct + (above * span_pct / span_mv) as u8;
        }
    }
    0
}

// ── Battery full (gateway side) ─────────────────────────────────────────────

/// On USB the charger holds the cell at ~4.2 V from the start of the
/// constant-voltage phase (~85 %) until it terminates, and the charge LED is
/// not wired to a GPIO. So "full" is taken as: on USB power and at least
/// [`FULL_MV`] for [`FULL_AFTER_SECS`]. Once full, it stays full down to
/// [`FULL_HOLD_MV`], since a terminated cell relaxes a little below 4.2 V.
pub const FULL_MV: u16 = 4150;
pub const FULL_HOLD_MV: u16 = 4100;
pub const FULL_AFTER_SECS: u64 = 3600;

#[derive(Clone, Copy, Default)]
struct ChargeState {
    high_since: Option<u64>,
    full: bool,
}

impl ChargeState {
    fn update(&mut self, usb_power: bool, battery_mv: u16, now_secs: u64) -> bool {
        if !usb_power || battery_mv == 0 {
            *self = Self::default();
            return false;
        }
        let threshold = if self.full { FULL_HOLD_MV } else { FULL_MV };
        if battery_mv >= threshold {
            let since = *self.high_since.get_or_insert(now_secs);
            self.full = now_secs.saturating_sub(since) >= FULL_AFTER_SECS;
        } else {
            *self = Self::default();
        }
        self.full
    }
}

/// Per-sensor battery-full tracking by MAC, for a fixed number of sensors
/// (the oldest slot is reused). State lives only in RAM: after a gateway
/// restart the hour starts again.
pub struct BatteryFullTracker<const N: usize> {
    entries: [Option<([u8; 6], ChargeState)>; N],
    next_slot: usize,
}

impl<const N: usize> BatteryFullTracker<N> {
    pub const fn new() -> Self {
        Self { entries: [None; N], next_slot: 0 }
    }

    /// Feeds one reading; returns whether that sensor's battery is full.
    pub fn update(&mut self, mac: &[u8; 6], reading: &Reading, now_secs: u64) -> bool {
        let slot = match self.entries.iter().position(|e| matches!(e, Some((m, _)) if m == mac)) {
            Some(i) => i,
            None => {
                let i = self.next_slot;
                self.entries[i] = Some((*mac, ChargeState::default()));
                self.next_slot = (i + 1) % N;
                i
            }
        };
        let (_, state) = self.entries[slot].as_mut().unwrap();
        state.update(reading.usb_power(), reading.battery_mv, now_secs)
    }
}
