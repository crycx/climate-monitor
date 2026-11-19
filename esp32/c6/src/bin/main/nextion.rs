//! Nextion HMI display (NX4024T032: 400x240, landscape) on UART1, TX only,
//! on the OLED's pins: GPIO6 -> display RX (built with NEXTION_ENABLED=1).
//!
//! The Nextion project only needs one blank page and one font (id 0, about
//! 24 px high, ISO-8859-15). Everything is drawn from here with `xstr`, so
//! the layout lives in this file; see README "Nextion display".
//!
//! Screen: a header (title + IP / AP / no WiFi), then one row per sensor:
//! this gateway's own DHT22 first, then every ESP-NOW sensor heard since
//! boot. Columns: name, temperature, humidity, battery, time since the
//! last report. Sensors that missed about three reports turn grey.
//!
//! Only cells whose text or colour changed are sent, so the link is quiet
//! between readings apart from the age column.

use alloc::{format, string::String, vec::Vec};
use core::cell::RefCell;
use core::sync::atomic::Ordering;

use embassy_net::Stack;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use embassy_time::{Duration, Instant, Timer};
use esp_hal::{
    Async,
    uart::{Config, UartTx},
};

use crate::app_config::AppConfig;
use crate::espnow_gateway;
use crate::log_runtime_line;

/// Nextion factory default; switched to `RUN_BAUD` at start.
const BOOT_BAUD: u32 = 9600;
const RUN_BAUD: u32 = 115_200;
const REFRESH_MS: u64 = 1000;
/// Full redraw now and then, in case the display was power-cycled alone.
const FULL_REDRAW_SECS: u64 = 300;
/// The gateway's own reading turns grey after this long without a DHT22 read.
const LOCAL_STALE_SECS: u64 = 600;

const WIDTH: u16 = 400;
const HEIGHT: u16 = 240;
const ROW_H: u16 = 26;
const HEADER_H: u16 = 28;
const FONT: u8 = 0;
const MAX_ROWS: usize = ((HEIGHT - HEADER_H) / ROW_H) as usize;

// RGB565. White text on black, coloured readings; grey = stale sensor,
// red = battery low.
const WHITE: u16 = 0xFFFF;
const BLACK: u16 = 0x0000;
const GREY: u16 = 0x8410;
const RED: u16 = 0xF800;
const TEMP: u16 = 0xFE60; // amber
const HUM: u16 = 0x5D7F; // light blue

#[derive(Clone, Copy)]
enum Align {
    Left = 0,
    Right = 2,
}

/// (x, width, alignment) per column.
const COLUMNS: [(u16, u16, Align); 5] = [
    (6, 150, Align::Left),   // name
    (156, 80, Align::Right), // temperature
    (236, 60, Align::Right), // humidity
    (296, 52, Align::Right), // battery
    (348, 46, Align::Right), // age
];

static LOCAL: critical_section::Mutex<RefCell<Option<(f32, f32, Instant)>>> =
    critical_section::Mutex::new(RefCell::new(None));

/// The gateway's own DHT22 reading (after offsets).
pub fn set_local(temp: f32, hum: f32) {
    critical_section::with(|cs| *LOCAL.borrow_ref_mut(cs) = Some((temp, hum, Instant::now())));
}

#[derive(Clone, PartialEq)]
struct Cell {
    text: String,
    color: u16,
}

fn cell(text: String, color: u16) -> Cell {
    Cell { text, color }
}

fn format_age(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h", secs / 3600)
    }
}

fn row_cells(
    name: &str,
    temp: Option<f32>,
    hum: Option<f32>,
    battery_pct: Option<u8>,
    charging: bool,
    age_secs: u64,
    stale: bool,
) -> [Cell; 5] {
    let (text_c, temp_c, hum_c) = if stale { (GREY, GREY, GREY) } else { (WHITE, TEMP, HUM) };
    let battery_c = match battery_pct {
        Some(p) if p <= 10 && !stale => RED,
        _ => text_c,
    };
    [
        cell(name.into(), text_c),
        cell(temp.map(|t| format!("{:.1}\u{b0}C", t)).unwrap_or_else(|| "--".into()), temp_c),
        cell(hum.map(|h| format!("{:.0}%", h)).unwrap_or_else(|| "--".into()), hum_c),
        cell(
            if charging { "USB".into() } else { battery_pct.map(|p| format!("{}%", p)).unwrap_or_default() },
            battery_c,
        ),
        cell(format_age(age_secs), text_c),
    ]
}

async fn rows(config: &'static Mutex<CriticalSectionRawMutex, AppConfig>) -> Vec<[Cell; 5]> {
    let mut out = Vec::new();
    if let Some((temp, hum, at)) = critical_section::with(|cs| *LOCAL.borrow_ref(cs)) {
        let name = {
            let cfg = config.lock().await;
            let id = cfg.mqtt_device_id.trim();
            String::from(if id.is_empty() { "This device" } else { id })
        };
        let age = Instant::now().duration_since(at).as_secs();
        out.push(row_cells(name.as_str(), Some(temp), Some(hum), None, false, age, age > LOCAL_STALE_SECS));
    }
    for s in espnow_gateway::sensor_rows() {
        out.push(row_cells(s.name.as_str(), s.temp, s.hum, s.battery_pct, s.charging, s.age_secs, s.stale));
    }
    out.truncate(MAX_ROWS);
    out
}

fn header_status(stack: &Stack<'static>) -> String {
    if crate::AP_ACTIVE.load(Ordering::Relaxed) {
        "AP 192.168.4.1".into()
    } else if let Some(cfg) = stack.config_v4() {
        format!("{}", cfg.address.address())
    } else {
        "No WiFi".into()
    }
}

/// Text as the display's ISO-8859-15 font expects it. `"` and `\` would end
/// or escape the Nextion string, so they are replaced.
fn push_text(out: &mut Vec<u8>, text: &str) {
    for c in text.chars() {
        out.push(match c {
            '"' => b'\'',
            '\\' => b'/',
            ' '..='~' => c as u8,
            'Š' => 0xA6,
            'š' => 0xA8,
            'Ž' => 0xB4,
            'ž' => 0xB8,
            '\u{a4}' | '\u{a6}' | '\u{a8}' | '\u{b4}' | '\u{b8}' | '\u{bc}'..='\u{be}' => b'?',
            '\u{a0}'..='\u{ff}' => c as u32 as u8,
            _ => b'?',
        });
    }
}

struct Display {
    tx: UartTx<'static, Async>,
    buf: Vec<u8>,
}

impl Display {
    fn cmd(&mut self, text: &str) {
        self.buf.extend_from_slice(text.as_bytes());
        self.buf.extend_from_slice(&[0xFF, 0xFF, 0xFF]);
    }

    fn xstr(&mut self, x: u16, y: u16, w: u16, h: u16, color: u16, bg: u16, align: Align, text: &str) {
        // xstr x,y,w,h,font,fg,bg,xcenter,ycenter(1=middle),sta(1=solid bg),"text"
        self.buf.extend_from_slice(
            format!("xstr {},{},{},{},{},{},{},{},1,1,\"", x, y, w, h, FONT, color, bg, align as u8).as_bytes(),
        );
        push_text(&mut self.buf, text);
        self.buf.extend_from_slice(b"\"");
        self.buf.extend_from_slice(&[0xFF, 0xFF, 0xFF]);
    }

    async fn flush(&mut self) {
        let mut rest = self.buf.as_slice();
        while !rest.is_empty() {
            match self.tx.write_async(rest).await {
                Ok(n) => rest = &rest[n..],
                Err(_) => break,
            }
        }
        let _ = self.tx.flush_async().await;
        self.buf.clear();
    }
}

#[embassy_executor::task]
pub async fn nextion_task(
    tx: UartTx<'static, Async>,
    stack: &'static Stack<'static>,
    config: &'static Mutex<CriticalSectionRawMutex, AppConfig>,
) {
    // Give the display time to boot when both power up together.
    Timer::after(Duration::from_millis(800)).await;

    // A display at the factory 9600 baud switches to RUN_BAUD; one already at
    // RUN_BAUD (only the ESP restarted) ignores this as noise.
    let mut d = Display { tx, buf: Vec::new() };
    d.buf.extend_from_slice(&[0xFF, 0xFF, 0xFF]);
    d.cmd(format!("baud={}", RUN_BAUD).as_str());
    d.flush().await;
    Timer::after(Duration::from_millis(100)).await;
    if d.tx.apply_config(&Config::default().with_baudrate(RUN_BAUD)).is_err() {
        log_runtime_line("Nextion: baud switch failed");
    }
    log_runtime_line("Nextion: UART1 TX on GPIO6");

    let mut drawn: Vec<[Cell; 5]> = Vec::new();
    let mut header = String::new();
    let mut next_full_redraw = Instant::now();

    loop {
        if Instant::now() >= next_full_redraw {
            next_full_redraw = Instant::now() + Duration::from_secs(FULL_REDRAW_SECS);
            // No replies (we only transmit), no sleep, full brightness.
            d.buf.extend_from_slice(&[0xFF, 0xFF, 0xFF]);
            d.cmd("bkcmd=0");
            d.cmd("thsp=0");
            d.cmd("ussp=0");
            d.cmd("sleep=0");
            d.cmd("page 0");
            d.cmd(format!("cls {}", BLACK).as_str());
            // White divider under the header.
            d.cmd(format!("fill 0,{},{},1,{}", HEADER_H - 1, WIDTH, WHITE).as_str());
            d.xstr(6, 0, 200, HEADER_H - 1, WHITE, BLACK, Align::Left, "Climate Monitor");
            drawn.clear();
            header.clear();
        }

        let status = header_status(stack);
        if status != header {
            d.xstr(206, 0, WIDTH - 212, HEADER_H - 1, WHITE, BLACK, Align::Right, status.as_str());
            header = status;
        }

        let rows = rows(config).await;
        for (i, row) in rows.iter().enumerate() {
            let y = HEADER_H + 2 + i as u16 * ROW_H;
            for (c, cell) in row.iter().enumerate() {
                if drawn.get(i).is_some_and(|old| old[c] == *cell) {
                    continue;
                }
                let (x, w, align) = COLUMNS[c];
                d.xstr(x, y, w, ROW_H, cell.color, BLACK, align, cell.text.as_str());
            }
        }
        // Rows that disappeared (cannot happen today: sensors are kept until reboot).
        for i in rows.len()..drawn.len() {
            let y = HEADER_H + 2 + i as u16 * ROW_H;
            d.cmd(format!("fill 0,{},{},{},{}", y, WIDTH, ROW_H, BLACK).as_str());
        }
        drawn = rows;

        if !d.buf.is_empty() {
            d.flush().await;
        }
        Timer::after(Duration::from_millis(REFRESH_MS)).await;
    }
}

/// Builds the UART for `nextion_task` at the display's factory baud rate.
pub fn uart(
    uart1: esp_hal::peripherals::UART1<'static>,
    tx_pin: esp_hal::peripherals::GPIO6<'static>,
) -> Option<UartTx<'static, Async>> {
    match UartTx::new(uart1, Config::default().with_baudrate(BOOT_BAUD)) {
        Ok(tx) => Some(tx.with_tx(tx_pin).into_async()),
        Err(e) => {
            log_runtime_line(format!("Nextion: UART init failed: {:?}", e).as_str());
            None
        }
    }
}
