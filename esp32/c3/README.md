# ESP32-C3 Climate Sensor Firmware

Reads temperature and humidity from a **DHT22** sensor and posts the data over
HTTPS to the Climate Monitor backend API.

## Hardware

| Component | Detail |
|-----------|--------|
| Board | ESP32-C3 (RISC-V, e.g. ESP32-C3-DevKitM-1) |
| Sensor | DHT22 connected to **GPIO10** (open-drain, pull-up) |

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
rustup target add --toolchain nightly riscv32imc-unknown-none-elf
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
[target.riscv32imc-unknown-none-elf]
# Change COM3 to the port your board is connected to.
# On Linux/macOS use e.g. /dev/ttyUSB0 or /dev/cu.usbserial-0001
runner = "espflash flash --monitor --chip esp32c3 --port COM3"

[env]
WIFI_SSID     = "YourNetworkName"
WIFI_PASSWORD = "YourWifiPassword"
# Choose one API URL (trailing slash required):
# API_URL     = "https://climate.sinpal.ee/api/"     # production (HTTPS)
API_URL       = "http://192.168.50.25:8000/api/"     # local network (HTTP)
API_KEY       = "your-64-char-api-key-here"

[build]
target = "riscv32imc-unknown-none-elf"

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
   VALUES (1, 'DHT22-C3', 'dht22', 'your-64-char-api-key-here', true, NOW(), NOW());"
```

Replace `user_id` with the ID of the user who owns the device and update the
`api_key` value to match `.cargo/config.toml`.

## Project structure

```
esp32/c3/
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
| DHT22 always errors | Wiring | Check GPIO10 connection and pull-up resistor |
