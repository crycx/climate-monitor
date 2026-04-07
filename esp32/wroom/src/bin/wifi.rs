use esp_hal::delay::Delay;
use esp_radio::wifi::{ClientConfig, ModeConfig, ScanConfig, WifiController};

pub fn connect_to_wifi(
    controller: &mut WifiController<'static>,
    delay: &Delay,
    ssid: &str,
    password: &str,
) {
    controller
        .set_config(&ModeConfig::Client(
            ClientConfig::default()
                .with_ssid(ssid.into())
                .with_password(password.into()),
        ))
        .unwrap();

    controller.start().unwrap();

    delay.delay_millis(1000);

    loop {
        crate::logln!("Scanning for '{}'...", ssid);
        match controller.scan_with_config(ScanConfig::default()) {
            Ok(results) => {
                crate::logln!("Found {} networks:", results.len());
                for ap in &results {
                    crate::logln!(
                        "  SSID: {}, Signal: {}, Auth: {:?}",
                        ap.ssid,
                        ap.signal_strength,
                        ap.auth_method
                    );
                }

                if results.iter().any(|ap| ap.ssid == ssid) {
                    crate::logln!("Network '{}' found, attempting to connect...", ssid);
                    break;
                }
            }
            Err(error) => {
                crate::logln!("Scan failed: {:?}", error);
            }
        }

        crate::logln!("'{}' not found, rescanning in 3s...", ssid);
        delay.delay_millis(3000);
    }

    crate::logln!("Connecting to '{}'...", ssid);

    let mut retries = 0;
    loop {
        match controller.connect() {
            Ok(()) => {}
            Err(error) => {
                crate::logln!("connect() error: {:?}", error);
            }
        }

        delay.delay_millis(2000);

        match controller.is_connected() {
            Ok(true) => {
                crate::logln!("Wi-Fi connected!");
                break;
            }
            Ok(false) => {
                crate::logln!("Waiting for Wi-Fi... (attempt {})", retries);
            }
            Err(error) => {
                crate::logln!(
                    "Wi-Fi status: {:?}, retrying... (attempt {})",
                    error,
                    retries
                );
            }
        }

        retries += 1;
        if retries > 20 {
            crate::logln!("Failed to connect after 20 attempts, restarting...");
        }
    }
}
