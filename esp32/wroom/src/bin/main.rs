#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use embedded_dht_rs::dht22::Dht22;
use esp_backtrace as _;
use esp_radio::wifi::{ClientConfig, ModeConfig};
use esp_rtos as _;
use esp_hal::{
    clock::CpuClock,
    delay::Delay,
    gpio::{DriveMode, Flex, OutputConfig, Pull},
    main,
    timer::timg::TimerGroup,
};

// This creates a default app-descriptor required by the ESP-IDF bootloader.
esp_bootloader_esp_idf::esp_app_desc!();

// #[panic_handler]
// fn panic(_: &core::panic::PanicInfo) -> ! {
//     loop {}
// }

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[main]
fn main() -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(#[unsafe(link_section = ".dram2_uninit")] size: 64 * 1024);
    esp_alloc::heap_allocator!(size: 36 * 1024);
    esp_println::logger::init_logger_from_env();

    let delay = Delay::new();

    let timg0 = TimerGroup::new(peripherals.TIMG0);
      esp_rtos::start(timg0.timer0);
    let esp_radio_ctrl = esp_radio::init().unwrap();

    let (mut controller, _interfaces) =
        esp_radio::wifi::new(&esp_radio_ctrl, peripherals.WIFI, Default::default()).unwrap();

    controller
        .set_config(&ModeConfig::Client(
            ClientConfig::default()
                .with_ssid(WIFI_SSID.into())
                .with_password(WIFI_PASSWORD.into()),
        ))
        .unwrap();

    controller.start().unwrap();

    esp_println::println!("SSID: {}, PSWD: {}",
              WIFI_SSID, WIFI_PASSWORD);

    // loop {
    //   let scan_results = controller.scan_with_config(ScanConfig::default()).unwrap();
    //   esp_println::println!("Scan results:");
    //   for ap in scan_results {
    //       esp_println::println!("SSID: {}, Channel: {}, Signal{},  Auth: {:?}",
    //           ap.ssid, ap.channel, ap.signal_strength, ap.auth_method);
    //   }
    //   delay.delay_millis(2000);

    // }

    controller.connect().unwrap();

    loop {
      match controller.is_connected() {
          Ok(true) => {
              esp_println::println!("Wi-Fi connected");
              break;
          }
          Ok(false) => {
              esp_println::println!("Waiting for Wi-Fi...");
          }
          Err(err) => {
              esp_println::println!("Wi-Fi status error: {:?}", err);
          }
      }
          delay.delay_millis(500);
    }

    esp_println::println!("Wi-Fi started, SSID configured");
    let mut dht22_pin = Flex::new(peripherals.GPIO4);
    dht22_pin.apply_output_config(
        &OutputConfig::default()
            .with_drive_mode(DriveMode::OpenDrain)
            .with_pull(Pull::None),
    );
    dht22_pin.set_output_enable(true);
    dht22_pin.set_input_enable(true);
    dht22_pin.set_high();

    let mut dht22 = Dht22::new(dht22_pin, Delay::new());

    loop {
        delay.delay_millis(2000);


        match dht22.read() {
            Ok(sensor_reading) => {
                esp_println::println!(
                    "DHT 22 Sensor - Temperature: {} °C, humidity: {} %",
                    sensor_reading.temperature,
                    sensor_reading.humidity
                );
            }
            Err(error) => {
                esp_println::dbg!("An error occurred while trying to read sensor: {:?}", error);
            }
        }
    }
}