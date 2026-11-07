//! USB console: active while the sensor is attached to a computer.
//!
//! Line-based commands over the USB serial port (any terminal, `espflash
//! monitor`, or `tools/config.html` in Chrome/Edge via Web Serial). The sensor
//! keeps measuring and sending every interval meanwhile. When the computer
//! disconnects it returns to battery mode.

use alloc::{string::String, vec::Vec};

use embassy_time::{Duration, Instant, Timer};
use esp_hal::{peripherals::{LPWR, USB_DEVICE}, usb_serial_jtag::UsbSerialJtag};
use esp_println::{print, println};

use crate::config::{ConfigStore, NodeConfig};
use crate::hw::{self, Measurement, NodeState, Radio, Sensors};

const MAX_LINE: usize = 160;
/// No SOF frames for this long = cable unplugged / host gone.
const HOST_GONE_MS: u64 = 2000;
const POLL_MS: u64 = 20;

const HELP: &str = "\
Commands:
  show                    current settings and status
  set <setting> <value>   change a setting (not saved yet)
      name <text>         HA device name (max 24 bytes)
      interval <secs>     10..21600
      temp_offset <C>     e.g. -0.5
      hum_offset <%>      e.g. 2
      key <secret>        ESPNOW_KEY, must match the gateway
      antenna internal|external
      warmup <ms>         DHT22 settle time, 0..5000
      battery on|off      battery measurement on D0
      divider <ratio>     battery divider ratio, default 2.0
  save                    store settings in flash
  defaults                erase saved settings (build defaults after reboot)
  test                    measure and send now
  reboot                  restart";

struct Console {
    cfg: NodeConfig,
    saved: NodeConfig,
    store: ConfigStore,
    sensors: Sensors,
    radio: Radio,
    state: NodeState,
    last: Option<(Measurement, bool)>,
    next_send_at: Instant,
}

impl Console {
    fn show(&self) {
        for (key, value) in self.cfg.describe() {
            println!("{}={}", key, value);
        }
        println!("unsaved={}", if self.cfg != self.saved { "yes" } else { "no" });
        println!("mac={}", hw::mac_str(&self.radio.own_mac));
        if self.state.channel != 0 {
            println!("gateway={}", hw::mac_str(&self.state.gateway));
            println!("channel={}", self.state.channel);
        } else {
            println!("gateway=none");
        }
        if let Some((m, delivered)) = &self.last {
            match m.climate {
                Some((t, h)) => {
                    println!("temperature={:.1}", t);
                    println!("humidity={:.1}", h);
                }
                None => println!("temperature=error"),
            }
            println!("battery_mv={}", m.battery_mv);
            println!("delivered={}", if *delivered { "yes" } else { "no" });
        }
        if !self.store.available() {
            println!("warning=no config partition, settings cannot be saved");
        }
    }

    async fn send_now(&mut self) {
        let result =
            hw::measure_and_send(&self.cfg, &mut self.sensors, &mut self.radio, &mut self.state).await;
        self.last = Some(result);
        self.next_send_at = Instant::now() + Duration::from_secs(self.cfg.interval_secs as u64);
    }

    async fn handle(&mut self, line: &str) {
        let line = line.trim();
        let (command, rest) = line.split_once(' ').unwrap_or((line, ""));
        match command {
            "" => return,
            "help" | "?" => println!("{}", HELP),
            "show" => self.show(),
            "set" => {
                let (field, value) = rest.trim().split_once(' ').unwrap_or((rest.trim(), ""));
                if let Err(e) = self.cfg.set(field, value) {
                    println!("ERR {}", e);
                    return;
                }
                if field == "interval" {
                    self.next_send_at = Instant::now() + Duration::from_secs(self.cfg.interval_secs as u64);
                }
                if field == "antenna" {
                    println!("note=antenna change applies after reboot");
                }
            }
            "save" => match self.store.save(&self.cfg) {
                Ok(()) => {
                    self.saved = self.cfg.clone();
                    println!("saved=yes");
                }
                Err(e) => {
                    println!("ERR {}", e);
                    return;
                }
            },
            "defaults" => match self.store.clear() {
                Ok(()) => {
                    self.cfg = NodeConfig::defaults();
                    self.saved = self.cfg.clone();
                    println!("note=saved settings erased, build defaults active");
                }
                Err(e) => {
                    println!("ERR {}", e);
                    return;
                }
            },
            "test" => {
                self.send_now().await;
                self.show();
            }
            "reboot" => {
                println!("OK");
                Timer::after(Duration::from_millis(100)).await;
                esp_hal::system::software_reset();
            }
            _ => {
                println!("ERR unknown command '{}' (try help)", command);
                return;
            }
        }
        println!("OK");
    }
}

pub async fn run(
    cfg: NodeConfig,
    store: ConfigStore,
    sensors: Sensors,
    radio: Radio,
    usb: USB_DEVICE<'static>,
    lpwr: LPWR<'static>,
) -> ! {
    let mut usb = UsbSerialJtag::new(usb);
    let mut console = Console {
        saved: cfg.clone(),
        cfg,
        store,
        sensors,
        radio,
        state: NodeState::load_or_init(),
        last: None,
        next_send_at: Instant::now(),
    };

    println!();
    println!("== Climate sensor USB console (type 'help') ==");
    console.show();
    println!("OK");

    // Raw bytes: UTF-8 names arrive split across reads; validated on Enter.
    let mut line: Vec<u8> = Vec::new();
    let mut last_sof = hw::usb_sof_frame();
    let mut last_sof_change = Instant::now();
    loop {
        let sof = hw::usb_sof_frame();
        if sof != last_sof {
            last_sof = sof;
            last_sof_change = Instant::now();
        } else if last_sof_change.elapsed() > Duration::from_millis(HOST_GONE_MS) {
            // On battery now: continue the normal cycle.
            console.state.store();
            hw::deep_sleep(lpwr, console.cfg.interval_secs as u64);
        }

        while let Ok(byte) = usb.read_byte() {
            match byte {
                b'\r' | b'\n' => {
                    if !line.is_empty() {
                        println!();
                        match String::from_utf8(core::mem::take(&mut line)) {
                            Ok(command) => console.handle(&command).await,
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
                b if line.len() >= MAX_LINE || (b.is_ascii_control()) => {}
                b => {
                    line.push(b);
                    // Echo ASCII; multi-byte characters are not echoed.
                    if b.is_ascii() {
                        print!("{}", b as char);
                    }
                }
            }
        }

        if Instant::now() >= console.next_send_at {
            console.send_now().await;
        }
        Timer::after(Duration::from_millis(POLL_MS)).await;
    }
}
