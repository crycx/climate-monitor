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
//!   20      flags        u8   bit0 = sensor read failed
//!   21      name_len     u8   0..=MAX_NAME_LEN
//!   22..    name         UTF-8, name_len bytes
//!   ..+8    tag
//!
//! Ack
//!   0..4    magic, version, kind = 3
//!   4..8    boot_id      echoed from the reading
//!   8..12   seq          echoed from the reading
//!   12      channel      gateway WiFi channel
//!   13..21  tag
//! ```

#![allow(dead_code)]

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

pub const MAGIC: [u8; 2] = *b"CM";
pub const VERSION: u8 = 1;
pub const TAG_LEN: usize = 8;
pub const MAX_NAME_LEN: usize = 24;
const READING_HEADER_LEN: usize = 22;
const ACK_BODY_LEN: usize = 13;
pub const MAX_PACKET_LEN: usize = READING_HEADER_LEN + MAX_NAME_LEN + TAG_LEN;

pub const FLAG_SENSOR_ERROR: u8 = 0x01;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    /// Plain reading, delivered by unicast to a known gateway.
    Reading = 1,
    /// Reading broadcast while searching for the gateway; asks for an [`Ack`].
    Discover = 2,
    /// Gateway reply to a [`Kind::Discover`] packet.
    Ack = 3,
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
    name: [u8; MAX_NAME_LEN],
    name_len: u8,
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
            name: [0; MAX_NAME_LEN],
            name_len: 0,
        }
    }

    /// Stores `name`, truncated to [`MAX_NAME_LEN`] bytes on a char boundary.
    pub fn set_name(&mut self, name: &str) {
        let mut end = name.len().min(MAX_NAME_LEN);
        while end > 0 && !name.is_char_boundary(end) {
            end -= 1;
        }
        self.name = [0; MAX_NAME_LEN];
        self.name[..end].copy_from_slice(&name.as_bytes()[..end]);
        self.name_len = end as u8;
    }

    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len as usize]).unwrap_or("")
    }

    pub fn sensor_ok(&self) -> bool {
        self.flags & FLAG_SENSOR_ERROR == 0
    }

    pub fn temperature_c(&self) -> f32 {
        self.temp_centi as f32 / 100.0
    }

    pub fn humidity_pct(&self) -> f32 {
        self.hum_centi as f32 / 100.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ack {
    pub boot_id: u32,
    pub seq: u32,
    pub channel: u8,
}

#[derive(Clone, Copy, Debug)]
pub enum Packet {
    Reading { reading: Reading, wants_ack: bool },
    Ack(Ack),
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
    debug_assert!(kind != Kind::Ack);
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
    append_tag(key, src_mac, out, READING_HEADER_LEN + name_len)
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
            if name_len > MAX_NAME_LEN || data.len() != READING_HEADER_LEN + name_len + TAG_LEN {
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
                flags: data[20],
                name: [0; MAX_NAME_LEN],
                name_len: 0,
            };
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
        _ => Err(DecodeError::UnknownKind),
    }
}

/// Rough LiPo state of charge from cell voltage under light load.
pub fn lipo_percent(battery_mv: u16) -> u8 {
    const CURVE: [(u16, u8); 11] = [
        (4200, 100),
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
