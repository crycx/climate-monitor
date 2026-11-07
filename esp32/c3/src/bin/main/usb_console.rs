//! USB console: change settings from a computer instead of the AP portal.
//!
//! Runs as a background task in both transports (WiFi and ESP-NOW). Commands
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

use crate::app_config::{self, AppConfig, FlashConfigStorage};

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
  transport wifi|espnow    espnow_key <secret>   espnow_name <text>
  interval <5..3600 s>     temp_offset <C>       hum_offset <%>
  deep_sleep on|off        power_source battery|power_bank
  oled_orientation landscape|portrait
  wifi_ssid[_2.._5] <ssid> wifi_password[_2.._5] <pw> wifi_location[_2.._5] <text>
  mqtt_enabled[_2.._5] on|off
  mqtt_host  mqtt_port  mqtt_username  mqtt_password  mqtt_base_topic  mqtt_device_id
  api_url  api_key";

/// Settable fields, in `show` order.
const FIELDS: &[&str] = &[
    "transport", "espnow_key", "espnow_name", "interval", "temp_offset", "hum_offset", "deep_sleep",
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
    Some(match name {
        "transport" => Text(&mut cfg.transport),
        "espnow_key" => Secret(&mut cfg.espnow_key),
        "espnow_name" => Text(&mut cfg.espnow_name),
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
    match name {
        "transport" => match value {
            app_config::TRANSPORT_WIFI | app_config::TRANSPORT_ESPNOW => Ok(value.into()),
            _ => Err("expected wifi or espnow".into()),
        },
        "espnow_key" => {
            if value.len() < app_config::ESPNOW_KEY_MIN_LEN || value.len() > 64 || value.contains(' ') {
                Err(format!("key must be {}..64 characters without spaces", app_config::ESPNOW_KEY_MIN_LEN))
            } else {
                Ok(value.into())
            }
        }
        "espnow_name" if value.len() > 24 => Err("max 24 bytes".into()),
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
    for name in FIELDS {
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
    println!("info.running_transport={}", saved.transport);
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
