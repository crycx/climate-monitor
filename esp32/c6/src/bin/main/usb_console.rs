//! USB console: change settings from a computer instead of the AP portal.
//!
//! Runs as a background task next to the normal runtime. Commands
//! are typed in any serial terminal (`espflash monitor`) or sent by
//! `tools/config.html` (Chrome/Edge, Web Serial). Every command ends with a
//! line `OK` or `ERR <reason>`.
//!
//! `show` output, one per line:
//! - `cfg.<field>=<value>`   editable setting
//! - `secret.<field>=yes|no` write-only setting (password/key), set or not
//! - `info.<key>=<value>`    read-only status
//!
//! `save` writes flash and restarts, so every setting takes effect the same
//! way as after saving in the portal.

use alloc::{format, string::String, vec::Vec};

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use embassy_time::{Duration, Instant, Timer};
use esp_hal::{Blocking, peripherals::USB_DEVICE, usb_serial_jtag::UsbSerialJtag};
use esp_println::{print, println};

use crate::app_config::{self, AppConfig, ESPNOW_SENSOR_SLOTS, FlashConfigStorage};
use crate::espnow_gateway;

const MAX_LINE: usize = 200;
const POLL_MS: u64 = 20;
/// Without SOF frames for this long there is no computer on the USB port.
const HOST_SOF_WINDOW_MS: u64 = 5;

const HELP: &str = "\
Commands:
  show                     settings and status
  set <field> <value>      change a setting (empty value clears it)
  save                     store in flash and restart
  discard                  drop unsaved changes
  reboot                   restart without saving
Fields:
  interval <5..3600 s>     temp_offset <C>       hum_offset <%>
  deep_sleep on|off        power_source battery|power_bank
  oled_orientation landscape|portrait
  wifi_ssid[_2.._5] <ssid> wifi_password[_2.._5] <pw> wifi_location[_2.._5] <text>
  mqtt_enabled[_2.._5] on|off
  mqtt_host  mqtt_port  mqtt_username  mqtt_password  mqtt_base_topic  mqtt_device_id
  api_url  api_key
  espnow_sensor_mac_<1..8> <mac>        ESP-NOW sensor to forward to the backend
  espnow_sensor_api_key_<1..8> <key>    that sensor's backend device API key";

/// Settable fields, in `show` order.
const FIELDS: &[&str] = &[
    "interval", "temp_offset", "hum_offset", "deep_sleep",
    "power_source", "oled_orientation",
    "wifi_ssid", "wifi_password", "wifi_location", "mqtt_enabled",
    "wifi_ssid_2", "wifi_password_2", "wifi_location_2", "mqtt_enabled_2",
    "wifi_ssid_3", "wifi_password_3", "wifi_location_3", "mqtt_enabled_3",
    "wifi_ssid_4", "wifi_password_4", "wifi_location_4", "mqtt_enabled_4",
    "wifi_ssid_5", "wifi_password_5", "wifi_location_5", "mqtt_enabled_5",
    "mqtt_host", "mqtt_port", "mqtt_username", "mqtt_password", "mqtt_base_topic", "mqtt_device_id",
    "api_url", "api_key",
];

enum Field<'a> {
    Text(&'a mut String),
    Secret(&'a mut String),
    Flag(&'a mut bool),
}

fn field<'a>(cfg: &'a mut AppConfig, name: &str) -> Option<Field<'a>> {
    use Field::*;
    if let Some(slot) = espnow_slot(name, "espnow_sensor_mac_") {
        return Some(Text(&mut cfg.espnow_sensor_mac[slot]));
    }
    if let Some(slot) = espnow_slot(name, "espnow_sensor_api_key_") {
        return Some(Secret(&mut cfg.espnow_sensor_api_key[slot]));
    }
    Some(match name {
        "interval" => Text(&mut cfg.sensor_interval_secs),
        "temp_offset" => Text(&mut cfg.temp_offset),
        "hum_offset" => Text(&mut cfg.hum_offset),
        "deep_sleep" => Flag(&mut cfg.deep_sleep_enabled),
        "power_source" => Text(&mut cfg.power_source),
        "oled_orientation" => Text(&mut cfg.oled_orientation),
        "wifi_ssid" => Text(&mut cfg.wifi_ssid),
        "wifi_password" => Secret(&mut cfg.wifi_password),
        "wifi_location" => Text(&mut cfg.wifi_location),
        "mqtt_enabled" | "mqtt_enabled_1" => Flag(&mut cfg.mqtt_enabled_1),
        "wifi_ssid_2" => Text(&mut cfg.wifi_ssid_2),
        "wifi_password_2" => Secret(&mut cfg.wifi_password_2),
        "wifi_location_2" => Text(&mut cfg.wifi_location_2),
        "mqtt_enabled_2" => Flag(&mut cfg.mqtt_enabled_2),
        "wifi_ssid_3" => Text(&mut cfg.wifi_ssid_3),
        "wifi_password_3" => Secret(&mut cfg.wifi_password_3),
        "wifi_location_3" => Text(&mut cfg.wifi_location_3),
        "mqtt_enabled_3" => Flag(&mut cfg.mqtt_enabled_3),
        "wifi_ssid_4" => Text(&mut cfg.wifi_ssid_4),
        "wifi_password_4" => Secret(&mut cfg.wifi_password_4),
        "wifi_location_4" => Text(&mut cfg.wifi_location_4),
        "mqtt_enabled_4" => Flag(&mut cfg.mqtt_enabled_4),
        "wifi_ssid_5" => Text(&mut cfg.wifi_ssid_5),
        "wifi_password_5" => Secret(&mut cfg.wifi_password_5),
        "wifi_location_5" => Text(&mut cfg.wifi_location_5),
        "mqtt_enabled_5" => Flag(&mut cfg.mqtt_enabled_5),
        "mqtt_host" => Text(&mut cfg.mqtt_host),
        "mqtt_port" => Text(&mut cfg.mqtt_port),
        "mqtt_username" => Text(&mut cfg.mqtt_username),
        "mqtt_password" => Secret(&mut cfg.mqtt_password),
        "mqtt_base_topic" => Text(&mut cfg.mqtt_base_topic),
        "mqtt_device_id" => Text(&mut cfg.mqtt_device_id),
        "api_url" => Text(&mut cfg.api_url),
        "api_key" => Secret(&mut cfg.api_key),
        _ => return None,
    })
}

/// "espnow_sensor_mac_3" -> Some(2) for prefix "espnow_sensor_mac_".
fn espnow_slot(name: &str, prefix: &str) -> Option<usize> {
    let n: usize = name.strip_prefix(prefix)?.parse().ok()?;
    (1..=ESPNOW_SENSOR_SLOTS).contains(&n).then(|| n - 1)
}

fn parse_flag(value: &str) -> Result<bool, String> {
    match value.to_ascii_lowercase().as_str() {
        "on" | "1" | "yes" | "true" => Ok(true),
        "off" | "0" | "no" | "false" => Ok(false),
        _ => Err("expected on or off".into()),
    }
}

/// Field-specific checks, mirroring what the portal accepts.
fn validate(name: &str, value: &str) -> Result<String, String> {
    let value = value.trim();
    if name.starts_with("espnow_sensor_mac_") {
        if value.is_empty() {
            return Ok(String::new());
        }
        return app_config::normalize_mac(value).ok_or_else(|| "expected a MAC like ac:a7:04:bb:5b:9c".into());
    }
    if name.starts_with("espnow_sensor_api_key_") && !value.is_empty() && value.len() != 64 {
        return Err("backend API keys are 64 characters".into());
    }
    match name {
        "interval" => match value.parse::<u64>() {
            Ok(secs) if (5..=3600).contains(&secs) => Ok(format!("{}", secs)),
            _ => Err("expected 5..3600 seconds".into()),
        },
        "temp_offset" | "hum_offset" => match value.parse::<f32>() {
            Ok(v) if (-20.0..=20.0).contains(&v) => Ok(value.into()),
            _ => Err("expected a number between -20 and 20".into()),
        },
        "power_source" => match value {
            "battery" | "power_bank" => Ok(value.into()),
            _ => Err("expected battery or power_bank".into()),
        },
        "oled_orientation" => match value {
            "landscape" | "portrait" => Ok(value.into()),
            _ => Err("expected landscape or portrait".into()),
        },
        "mqtt_port" => match value.parse::<u16>() {
            Ok(p) if p > 0 => Ok(value.into()),
            _ => Err("expected a port number".into()),
        },
        "api_url" if !value.is_empty() && !value.ends_with('/') => {
            Err("API URL must end with '/'".into())
        }
        _ => Ok(value.into()),
    }
}

fn set(cfg: &mut AppConfig, name: &str, value: &str) -> Result<(), String> {
    let Some(target) = field(cfg, name) else {
        return Err(format!("unknown field '{}' (try help)", name));
    };
    match target {
        Field::Flag(flag) => *flag = parse_flag(value.trim())?,
        Field::Text(text) | Field::Secret(text) => *text = validate(name, value)?,
    }
    Ok(())
}

fn show(pending: &AppConfig, saved: &AppConfig) {
    let mut cfg = pending.clone();
    let mut names: Vec<String> = FIELDS.iter().map(|n| String::from(*n)).collect();
    for i in 1..=ESPNOW_SENSOR_SLOTS {
        names.push(format!("espnow_sensor_mac_{}", i));
        names.push(format!("espnow_sensor_api_key_{}", i));
    }
    for name in names.iter().map(String::as_str) {
        match field(&mut cfg, name) {
            Some(Field::Text(text)) => println!("cfg.{}={}", name, text),
            Some(Field::Flag(flag)) => println!("cfg.{}={}", name, if *flag { "on" } else { "off" }),
            Some(Field::Secret(secret)) => {
                println!("secret.{}={}", name, if secret.is_empty() { "no" } else { "yes" })
            }
            None => {}
        }
    }
    println!("info.unsaved={}", if pending != saved { "yes" } else { "no" });
    // Sensors heard since boot, to copy MACs from: "mac,name,seconds ago".
    for (i, (mac, name, age)) in espnow_gateway::seen_sensors().iter().enumerate() {
        let mac = espnow_gateway::mac_hex(mac);
        let mapped = saved.espnow_backend_api_key(&mac).is_some();
        println!("info.seen_{}={},{},{},{}", i + 1, mac, name, age, if mapped { "backend" } else { "mqtt-only" });
    }
}

/// True while a computer (not a charger) is attached: the USB host sends a
/// start-of-frame every 1 ms.
pub fn usb_host_present() -> bool {
    let read = || USB_DEVICE::regs().fram_num().read().sof_frame_index().bits();
    let first = read();
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(HOST_SOF_WINDOW_MS) {
        if read() != first {
            return true;
        }
    }
    false
}

async fn handle(
    line: &str,
    pending: &mut AppConfig,
    base: &mut AppConfig,
    config: &'static Mutex<CriticalSectionRawMutex, AppConfig>,
    storage: &'static Mutex<CriticalSectionRawMutex, FlashConfigStorage<'static>>,
) {
    let line = line.trim();
    let (command, rest) = line.split_once(' ').unwrap_or((line, ""));
    let saved = config.lock().await.clone();
    // Saved elsewhere (portal) while nothing was edited here: follow it.
    if *pending == *base && saved != *base {
        *pending = saved.clone();
        *base = saved.clone();
    }
    match command {
        "" => return,
        "help" | "?" => println!("{}", HELP),
        "show" => show(pending, &saved),
        "set" => {
            let rest = rest.trim_start();
            let (name, value) = rest.split_once(' ').unwrap_or((rest, ""));
            if let Err(e) = set(pending, name, value) {
                println!("ERR {}", e);
                return;
            }
        }
        "discard" => {
            *pending = saved.clone();
            *base = saved;
        }
        "save" => {
            if let Err(e) = storage.lock().await.store_config(pending).await {
                println!("ERR store failed: {}", e);
                return;
            }
            println!("info.saved=yes");
            println!("info.restarting=yes");
            println!("OK");
            Timer::after(Duration::from_millis(200)).await;
            esp_hal::system::software_reset();
        }
        "reboot" => {
            println!("OK");
            Timer::after(Duration::from_millis(200)).await;
            esp_hal::system::software_reset();
        }
        _ => {
            println!("ERR unknown command '{}' (try help)", command);
            return;
        }
    }
    println!("OK");
}

#[embassy_executor::task]
pub async fn console_task(
    mut usb: UsbSerialJtag<'static, Blocking>,
    config: &'static Mutex<CriticalSectionRawMutex, AppConfig>,
    storage: &'static Mutex<CriticalSectionRawMutex, FlashConfigStorage<'static>>,
) {
    let mut pending = config.lock().await.clone();
    let mut base = pending.clone();
    println!("USB console ready (type 'help').");

    // Raw bytes: UTF-8 text can arrive split across reads; validated on Enter.
    let mut line: Vec<u8> = Vec::new();
    loop {
        while let Ok(byte) = usb.read_byte() {
            match byte {
                b'\r' | b'\n' => {
                    if !line.is_empty() {
                        println!();
                        match String::from_utf8(core::mem::take(&mut line)) {
                            Ok(command) => handle(&command, &mut pending, &mut base, config, storage).await,
                            Err(_) => println!("ERR invalid UTF-8 input"),
                        }
                    }
                }
                // Backspace / DEL: drop one whole UTF-8 character.
                0x08 | 0x7f => {
                    while line.last().is_some_and(|b| b & 0xC0 == 0x80) {
                        line.pop();
                    }
                    if line.pop().is_some() {
                        print!("\x08 \x08");
                    }
                }
                b if line.len() >= MAX_LINE || b.is_ascii_control() => {}
                b => {
                    line.push(b);
                    // Echo ASCII only; multi-byte characters are not echoed.
                    if b.is_ascii() {
                        print!("{}", b as char);
                    }
                }
            }
        }
        Timer::after(Duration::from_millis(POLL_MS)).await;
    }
}
