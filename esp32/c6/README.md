# ESP32-C6 Climate Sensor Firmware

Reads temperature and humidity from a **DHT22** sensor and posts the data over
HTTPS to the Climate Monitor backend API.

## Hardware

| Component | Detail |
|-----------|--------|
| Board | ESP32-C6 (RISC-V, e.g. ESP32-C6-DevKitC-1) |
| Sensor | DHT22 data connected to **GPIO5** (open-drain, pull-up) |
| Display | 0.91" OLED SSD1306 (I2C): **SDA GPIO6**, **SCL/SCK GPIO7** |

### Wiring

#### DHT22

- VCC -> 3V3 (recommended)
- GND -> GND
- DATA -> GPIO5
- 10k pull-up resistor between DATA and 3V3 (if your DHT board does not include one)

#### OLED 0.91" (SSD1306, I2C)

- GND -> GND
- VCC -> 3V3
- SDA -> GPIO6
- SCK (same as SCL on I2C modules) -> GPIO7

### Electrical safety note

If DHT22 is powered from 5V, do not connect DATA directly to ESP32-C6 GPIO.
ESP32-C6 pins are not 5V tolerant. Either power DHT22 from 3V3, or use a level shifter on DATA.

## Prerequisites

### 1. Install Rust (nightly)

```bash
# Install rustup if not already installed
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install the nightly toolchain (required by this project)
rustup toolchain install nightly

# Required for build-std in .cargo/config.toml
rustup component add --toolchain nightly rust-src

# Add the RISC-V bare-metal target
rustup target add --toolchain nightly riscv32imac-unknown-none-elf
```

On Windows use the [rustup installer](https://rustup.rs/) and then run the
`rustup` commands above in PowerShell.

### 2. Install espflash

`espflash` is used to flash and monitor the device.

```bash
cargo install espflash
```

### 3. USB driver (Windows only)

Install the **CP210x** or **CH340** USB-to-UART driver depending on your board.
The device will appear as a `COMx` port in Device Manager.

## Configuration

Edit `.cargo/config.toml` before building:

```toml
[target.riscv32imac-unknown-none-elf]
# Change COM3 to the port your board is connected to.
# On Linux/macOS use e.g. /dev/ttyUSB0 or /dev/cu.usbserial-0001
runner = "espflash flash --monitor --chip esp32c6 --port COM3 --partition-table partitions.csv --target-app-partition factory"

[env]
WIFI_SSID     = "YourNetworkName"
WIFI_PASSWORD = "YourWifiPassword"
# Optional: up to 5 saved WiFi networks for automatic fallback.
# The device will prefer a configured SSID that is visible in a pre-scan.
# If the first connect times out, it will try the other saved networks before
# opening the AP recovery portal.
# WIFI_SSID_2     = "BackupNetwork"
# WIFI_PASSWORD_2 = "BackupPassword"
# WIFI_LOCATION_2 = "garage"
# Optional AP portal tuning
# AP_CHANNEL = "6"               # 1..11 (often 6 or 11 are best)
# AP_PORTAL_PASSWORD = "12345678" # WPA2 password, min 8 chars
# Choose one API URL (trailing slash required):
# API_URL     = "https://climate.sinpal.ee/api/"     # production (HTTPS)
API_URL       = "http://192.168.50.25:8000/api/"     # local network (HTTP)
API_KEY       = "your-64-char-api-key-here"

[build]
target = "riscv32imac-unknown-none-elf"

[unstable]
build-std = ["core", "alloc"]
```

The `API_KEY` must match the `api_key` column of the corresponding row in the
`devices` table on the server.

## Build & Flash

### WiFi mode (sends data to server via HTTPS)

```bash
cargo run --release
```

This compiles, flashes, and opens a serial monitor in one step.
Press **Ctrl-R** to reset the board, **Ctrl-C** to exit the monitor.

### ESP32-C6 Super Mini RGB status LED

ESP32-C6 Super Mini uses an onboard **WS2812 RGB LED** on **GPIO8**.
Firmware drives it as a status indicator:

| State | LED color/pattern |
|------|--------------------|
| Booting | Blue (steady) |
| WiFi scanning/connecting | Amber/Orange (steady) |
| AP portal active | Purple (blinking) |
| Sending measurement to backend | Amber/Orange (steady) |
| Measurement sent successfully (short feedback) | Green (short steady pulse) |
| Idle (normal connected wait) | Off |

If you do not see the LED, verify you are running the latest flashed firmware and not an older binary.

### WiFi mode — local development (plain HTTP, no TLS)

Use this when the backend runs on your local network over HTTP.
Set `API_URL = "http://<your-server-ip>:<port>/api/"` in `.cargo/config.toml`, then:

```bash
cargo run --release --features no-tls
```

### Console-only mode (no WiFi, useful for sensor testing)

```bash
cargo run --release
```

DHT22 readings are printed to the serial monitor every 2 seconds.

## Serial output (WiFi mode)

```
WiFi: connecting to 'YourNetwork'...
WiFi: connected!
Waiting for IP...
Got IP: 192.168.1.42/24
DHT22: 23.40 °C  58.20 %
POST → HTTP 201
```

`HTTP 201` means the reading was saved successfully.
`HTTP 401` means the API key is wrong or the device row is missing in the DB.
`HTTP 400` means the request body was malformed (should not happen normally).

## Finding the serial port

**Windows** — open Device Manager → Ports (COM & LPT).

**Linux/macOS**
```bash
ls /dev/ttyUSB* /dev/cu.usbserial* 2>/dev/null
```

## Registering a new device on the server

A device row must exist in the `devices` table before the API key is accepted.
Run this on the server (or locally if the DB is accessible):

```bash
sudo docker compose -f docker-compose.no-registry.yaml exec db \
  psql -U postgres -d climate_monitor -P pager=off -c \
  "INSERT INTO devices (user_id, name, device_type, api_key, is_active, created_at, updated_at)
  VALUES (1, 'DHT22-C6', 'dht22', 'your-64-char-api-key-here', true, NOW(), NOW());"
```

Replace `user_id` with the ID of the user who owns the device and update the
`api_key` value to match `.cargo/config.toml`.

## Project structure

```
esp32/c6/
├── .cargo/
│   └── config.toml       # target, port, WiFi credentials, API key
├── src/
│   └── bin/
│       └── main.rs       # firmware entry point
├── Cargo.toml            # dependencies and feature flags
├── rust-toolchain.toml   # pins nightly toolchain
└── README.md             # this file
```

## Troubleshooting

| Symptom | Likely cause | Fix |
|---------|-------------|-----|
| `espflash: port not found` | Wrong port in `config.toml` | Update `--port` to match your COMx / ttyUSB |
| `cargo: feature no-tls not found` | Typo | Use `--features no-tls` (exact) |
| Flashes but no serial output | Monitor not opening | Run `espflash monitor --port COM3` separately |
| `WiFi: connect error` | Wrong SSID/password | Check `.cargo/config.toml` credentials |
| `POST → HTTP 401` | Missing/wrong API key | Insert device row in DB or fix `API_KEY` |
| `POST → HTTP 500` | Server error | Check server logs: `sudo docker compose logs api --tail 30` |
| DHT22 always errors | Wiring | Check GPIO5 connection and pull-up resistor |
| OLED blank screen | Wiring/address mismatch | Check GPIO6/GPIO7, then test OLED I2C address `0x3C` vs `0x3D` |
| Super Mini RGB LED stays dark | Older firmware on device or different board variant | Reflash latest build and verify board is ESP32-C6 Super Mini (WS2812 LED on GPIO8) |
