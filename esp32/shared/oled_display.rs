use core::fmt::Write;
use core::sync::atomic::{AtomicBool, AtomicU16, Ordering};

use display_interface::{DisplayError, WriteOnlyDataCommand};
use embedded_graphics::{
    Drawable,
    draw_target::DrawTarget,
    mono_font::{MonoTextStyle, ascii::{FONT_4X6, FONT_6X10}},
    pixelcolor::BinaryColor,
    prelude::Point,
    text::{Baseline, Text},
};
use heapless::String;
use ssd1306::{
    Ssd1306,
    mode::{BufferedGraphicsMode, DisplayConfig},
    prelude::{Brightness, DisplaySize128x32},
};

pub type OledDisplay<DI> = Ssd1306<DI, DisplaySize128x32, BufferedGraphicsMode<DisplaySize128x32>>;

const DISPLAY_WIDTH_PX: i32 = 128;
const FONT_4X6_CHAR_WIDTH_PX: i32 = 4;
const BRAND_TEXT: &str = "ClimateMonitor";
const OLED_PERIODIC_INIT_DRAWS: u16 = 300;
static DISPLAY_NEEDS_INIT: AtomicBool = AtomicBool::new(true);
static DRAW_COUNTER: AtomicU16 = AtomicU16::new(0);

fn brand_right_x() -> i32 {
    let text_width = BRAND_TEXT.len() as i32 * FONT_4X6_CHAR_WIDTH_PX;
    (DISPLAY_WIDTH_PX - text_width).max(0)
}

fn ensure_initialized<DI>(display: &mut OledDisplay<DI>) -> Result<(), DisplayError>
where
    DI: WriteOnlyDataCommand,
{
    if !DISPLAY_NEEDS_INIT.load(Ordering::Relaxed) {
        return Ok(());
    }

    display.init()?;
    display.set_brightness(Brightness::BRIGHTEST)?;
    DISPLAY_NEEDS_INIT.store(false, Ordering::Relaxed);
    esp_println::println!("OLED: initialized successfully");
    Ok(())
}

fn flush_resilient<DI>(display: &mut OledDisplay<DI>) -> Result<(), DisplayError>
where
    DI: WriteOnlyDataCommand,
{
    let next_draw = DRAW_COUNTER.load(Ordering::Relaxed).wrapping_add(1);
    DRAW_COUNTER.store(next_draw, Ordering::Relaxed);
    if next_draw >= OLED_PERIODIC_INIT_DRAWS {
        DRAW_COUNTER.store(0, Ordering::Relaxed);
        DISPLAY_NEEDS_INIT.store(true, Ordering::Relaxed);
        esp_println::println!("OLED: periodic init check (draw #{})",(next_draw));
    }

    // If init has failed previously (for example display unplugged at boot),
    // keep attempting init on each draw until it succeeds.
    ensure_initialized(display)?;

    // Retry up to 5 times before reinit. Transient I2C glitches caused by WiFi
    // Tx activity on ESP32-C3 typically resolve within 1-3 quick retries.
    // Reaching reinit too fast causes display.init() to send DISPLAYOFF, which
    // visibly blanks the screen before recovery — so we avoid it for transient errors.
    for _ in 0..5 {
        if display.flush().is_ok() {
            return Ok(());
        }
    }
    DISPLAY_NEEDS_INIT.store(true, Ordering::Relaxed);
    esp_println::println!("OLED: flush retry x5 failed, reinitializing display");

    // Last resort: reinitialize controller and resend current framebuffer.
    if let Err(e) = ensure_initialized(display) {
        esp_println::println!("OLED: reinit failed: {:?}", e);
        return Err(e);
    }
    let flush_result = display.flush();
    if flush_result.is_ok() {
        esp_println::println!("OLED: recovered after reinit");
    } else if let Err(ref e) = flush_result {
        esp_println::println!("OLED: flush still failing after reinit: {:?}", e);
    }
    flush_result
}

pub fn init_screen<DI>(display: &mut OledDisplay<DI>, status: &str) -> Result<(), DisplayError>
where
    DI: WriteOnlyDataCommand,
{
    DISPLAY_NEEDS_INIT.store(true, Ordering::Relaxed);
    ensure_initialized(display)?;
    draw_status(display, status)
}

pub fn draw_status<DI>(display: &mut OledDisplay<DI>, status: &str) -> Result<(), DisplayError>
where
    DI: WriteOnlyDataCommand,
{
    let text_style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let small_style = MonoTextStyle::new(&FONT_4X6, BinaryColor::On);

    let _ = display.clear(BinaryColor::Off);
    let _ = Text::with_baseline("Climate monitor", Point::new(0, 0), text_style, Baseline::Top)
        .draw(display);
    let _ = Text::with_baseline(status, Point::new(0, 11), text_style, Baseline::Top).draw(display);
    let _ = Text::with_baseline(BRAND_TEXT, Point::new(brand_right_x(), 26), small_style, Baseline::Top)
        .draw(display);
    flush_resilient(display)
}

pub fn prepare_for_sleep<DI>(display: &mut OledDisplay<DI>) -> Result<(), DisplayError>
where
    DI: WriteOnlyDataCommand,
{
    let text_style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let small_style = MonoTextStyle::new(&FONT_4X6, BinaryColor::On);

    display.set_brightness(Brightness::DIMMEST)?;
    let _ = display.clear(BinaryColor::Off);
    let _ = Text::with_baseline("Deep sleep", Point::new(0, 0), text_style, Baseline::Top)
        .draw(display);
    let _ = Text::with_baseline("Power save on", Point::new(0, 11), text_style, Baseline::Top)
        .draw(display);
    let _ = Text::with_baseline(BRAND_TEXT, Point::new(brand_right_x(), 26), small_style, Baseline::Top)
        .draw(display);
    let _ = flush_resilient(display);
    display.set_display_on(false)
}

#[allow(dead_code)]
pub fn show_reading<DI>(
    display: &mut OledDisplay<DI>,
    temp: f32,
    hum: f32,
    _hours: u8,
    _minutes: u8,
    data_sent: bool,
) -> Result<(), DisplayError>
where
    DI: WriteOnlyDataCommand,
{
    let small_style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let tiny_style = MonoTextStyle::new(&FONT_4X6, BinaryColor::On);
    let mut top_line: String<24> = String::new();
    let mut bottom_line: String<24> = String::new();

    let _ = write!(&mut top_line, "T:{:.1}C H:{:.1}%", temp, hum);
    if data_sent {
        let _ = write!(&mut bottom_line, "SENT");
    }

    let _ = display.clear(BinaryColor::Off);

    let _ = Text::with_baseline(top_line.as_str(), Point::new(0, 0), small_style, Baseline::Top)
        .draw(display);
    let _ = Text::with_baseline(bottom_line.as_str(), Point::new(0, 11), small_style, Baseline::Top)
        .draw(display);

    // Bottom right: "ClimateMonitor"
    let _ = Text::with_baseline(BRAND_TEXT, Point::new(brand_right_x(), 26), tiny_style, Baseline::Top)
        .draw(display);

    flush_resilient(display)
}

/// Font glyphs are ASCII only: map Estonian/common Latin letters to their base
/// letter, anything else non-ASCII to '?'.
#[allow(dead_code)]
fn push_ascii<const N: usize>(out: &mut String<N>, text: &str, max_chars: usize) {
    for ch in text.chars().take(max_chars) {
        let mapped = match ch {
            'õ' | 'ö' | 'ó' | 'ò' | 'ô' => 'o',
            'Õ' | 'Ö' | 'Ó' | 'Ò' | 'Ô' => 'O',
            'ä' | 'á' | 'à' | 'â' | 'å' => 'a',
            'Ä' | 'Á' | 'À' | 'Â' | 'Å' => 'A',
            'ü' | 'ú' | 'ù' | 'û' => 'u',
            'Ü' | 'Ú' | 'Ù' | 'Û' => 'U',
            'š' => 's',
            'Š' => 'S',
            'ž' => 'z',
            'Ž' => 'Z',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            c if c.is_ascii() => c,
            _ => '?',
        };
        if out.push(mapped).is_err() {
            break;
        }
    }
}

/// Page for a reading received from a remote (ESP-NOW) sensor.
///
/// Landscape:
/// ```text
/// <name>              <bat%>
/// T:21.4C H:45.2%
/// <footer, small font>
/// ```
#[allow(dead_code)]
pub fn show_remote_reading<DI>(
    display: &mut OledDisplay<DI>,
    name: &str,
    temp: Option<f32>,
    hum: Option<f32>,
    battery_pct: Option<u8>,
    footer: &str,
    is_portrait: bool,
) -> Result<(), DisplayError>
where
    DI: WriteOnlyDataCommand,
{
    const FONT_6X10_CHAR_WIDTH_PX: i32 = 6;
    let text_style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let small_style = MonoTextStyle::new(&FONT_4X6, BinaryColor::On);

    let mut battery: String<8> = String::new();
    if let Some(pct) = battery_pct {
        let _ = write!(&mut battery, "{}%", pct);
    }

    let _ = display.clear(BinaryColor::Off);

    if is_portrait {
        // 32 px wide: name in small font, values below.
        let mut name_line: String<12> = String::new();
        push_ascii(&mut name_line, name, 8);
        let mut temp_line: String<12> = String::new();
        let mut hum_line: String<12> = String::new();
        match (temp, hum) {
            (Some(t), Some(h)) => {
                let _ = write!(&mut temp_line, "{:.1}C", t);
                let _ = write!(&mut hum_line, "{:.1}%", h);
            }
            _ => {
                let _ = write!(&mut temp_line, "-");
                let _ = write!(&mut hum_line, "-");
            }
        }
        let _ = Text::with_baseline(name_line.as_str(), Point::new(0, 0), small_style, Baseline::Top)
            .draw(display);
        let _ = Text::with_baseline(temp_line.as_str(), Point::new(0, 8), text_style, Baseline::Top)
            .draw(display);
        let _ = Text::with_baseline(hum_line.as_str(), Point::new(0, 19), text_style, Baseline::Top)
            .draw(display);
        if !battery.is_empty() {
            let mut bat_line: String<12> = String::new();
            let _ = write!(&mut bat_line, "B:{}", battery.as_str());
            let _ = Text::with_baseline(bat_line.as_str(), Point::new(0, 30), small_style, Baseline::Top)
                .draw(display);
        }
        return flush_resilient(display);
    }

    // Name on the left, battery right-aligned on the same line.
    let battery_width = battery.len() as i32 * FONT_6X10_CHAR_WIDTH_PX;
    let name_max_chars = ((DISPLAY_WIDTH_PX - battery_width) / FONT_6X10_CHAR_WIDTH_PX - 1).max(1) as usize;
    let mut name_line: String<24> = String::new();
    push_ascii(&mut name_line, name, name_max_chars);

    let mut value_line: String<24> = String::new();
    match (temp, hum) {
        (Some(t), Some(h)) => {
            let _ = write!(&mut value_line, "T:{:.1}C H:{:.1}%", t, h);
        }
        _ => {
            let _ = write!(&mut value_line, "T:- H:- (sensor err)");
        }
    }

    let mut footer_line: String<40> = String::new();
    push_ascii(&mut footer_line, footer, (DISPLAY_WIDTH_PX / FONT_4X6_CHAR_WIDTH_PX) as usize);

    let _ = Text::with_baseline(name_line.as_str(), Point::new(0, 0), text_style, Baseline::Top)
        .draw(display);
    if !battery.is_empty() {
        let _ = Text::with_baseline(
            battery.as_str(),
            Point::new(DISPLAY_WIDTH_PX - battery_width, 0),
            text_style,
            Baseline::Top,
        )
        .draw(display);
    }
    let _ = Text::with_baseline(value_line.as_str(), Point::new(0, 11), text_style, Baseline::Top)
        .draw(display);
    let _ = Text::with_baseline(footer_line.as_str(), Point::new(0, 26), small_style, Baseline::Top)
        .draw(display);

    flush_resilient(display)
}

/// Show reading with optional WiFi SSID (for boot/connection phases)
#[allow(dead_code)]
pub fn show_reading_with_wifi<DI>(
    display: &mut OledDisplay<DI>,
    temp: Option<f32>,
    hum: Option<f32>,
    wifi_info: &str,
    data_sent: bool,
    scroll_tick: usize,
    is_portrait: bool,
) -> Result<(), DisplayError>
where
    DI: WriteOnlyDataCommand,
{
    let small_style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let compact_style = MonoTextStyle::new(&FONT_4X6, BinaryColor::On);
    let portrait_value_style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let mut top_line: String<24> = String::new();
    let mut bottom_line: String<24> = String::new();

    if is_portrait {
        let mut temp_value: String<12> = String::new();
        let mut hum_value: String<12> = String::new();

        match temp {
            Some(t) => {
                let _ = write!(&mut temp_value, "{:.1}C", t);
            }
            None => {
                let _ = write!(&mut temp_value, "-");
            }
        }

        match hum {
            Some(h) => {
                let _ = write!(&mut hum_value, "{:.1}%", h);
            }
            None => {
                let _ = write!(&mut hum_value, "-");
            }
        }

        let _ = display.clear(BinaryColor::Off);
        // Portrait: T at y=0, H at y=11, SENT at y=22 (10px spacing for FONT_6X10)
        let _ = Text::with_baseline("T:", Point::new(0, 0), compact_style, Baseline::Top)
            .draw(display);
        let _ = Text::with_baseline(temp_value.as_str(), Point::new(10, 0), portrait_value_style, Baseline::Top)
            .draw(display);

        let _ = Text::with_baseline("H:", Point::new(0, 11), compact_style, Baseline::Top)
            .draw(display);
        let _ = Text::with_baseline(hum_value.as_str(), Point::new(10, 11), portrait_value_style, Baseline::Top)
            .draw(display);

        if data_sent {
            let _ = Text::with_baseline("SENT", Point::new(0, 22), compact_style, Baseline::Top)
                .draw(display);
        }

        return flush_resilient(display);
    } else {
        match (temp, hum) {
            (Some(t), Some(h)) => {
                let _ = write!(&mut top_line, "T:{:.1}C H:{:.1}%", t, h);
            }
            _ => {
                let _ = write!(&mut top_line, "T:- H:-");
            }
        }

        let wifi_width = if data_sent { 16 } else { 21 };
        let mut wifi_prefix_and_ssid: String<40> = String::new();
        let _ = write!(&mut wifi_prefix_and_ssid, "{}", wifi_info);

        if wifi_prefix_and_ssid.len() <= wifi_width {
            let _ = write!(&mut bottom_line, "{}", wifi_prefix_and_ssid.as_str());
        } else {
            let mut marquee_src: String<48> = String::new();
            let _ = write!(&mut marquee_src, "{}   ", wifi_prefix_and_ssid.as_str());
            let bytes = marquee_src.as_bytes();
            let src_len = bytes.len();
            let start = if src_len == 0 { 0 } else { scroll_tick % src_len };

            for i in 0..wifi_width {
                let idx = (start + i) % src_len;
                let _ = bottom_line.push(bytes[idx] as char);
            }
        }

        if data_sent {
            let _ = write!(&mut bottom_line, " SENT");
        }
    }

    let _ = display.clear(BinaryColor::Off);

    let _ = Text::with_baseline(top_line.as_str(), Point::new(0, 0), small_style, Baseline::Top)
        .draw(display);
    let _ = Text::with_baseline(bottom_line.as_str(), Point::new(0, 11), small_style, Baseline::Top)
        .draw(display);
    let _ = Text::with_baseline(BRAND_TEXT, Point::new(brand_right_x(), 26), compact_style, Baseline::Top)
        .draw(display);

    flush_resilient(display)
}