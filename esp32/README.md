# ESP32 Firmware Overview

This folder contains two firmware targets and one shared code area:

- `c3/` for ESP32-C3 (RISC-V)
- `wroom/` for ESP32-WROOM (Xtensa ESP32)
- `shared/` for extracted reusable runtime and WiFi/AP config modules

Both board builds currently run as independent Cargo projects. Shared code is extracted, but full one-build-for-all integration is intentionally deferred.

## Home Assistant (MQTT)

ESP32-C3 firmware can publish readings to an MQTT broker and announce Home Assistant entities via MQTT Discovery.

- Configure MQTT in the device AP portal (http://192.168.4.1/) under **MQTT (Home Assistant)**.
- Once configured, Home Assistant will auto-create two sensors (temperature + humidity) via discovery.

Published topics (defaults):

- Availability: `climate_monitor/<device_id>/availability` (payload: `online`)
- Temperature: `climate_monitor/<device_id>/temperature`
- Humidity: `climate_monitor/<device_id>/humidity`

Discovery topics (retained):

- `homeassistant/sensor/<device_id>_temperature/config`
- `homeassistant/sensor/<device_id>_humidity/config`

## Directory Layout

```
esp32/
├── c3/
│   ├── .cargo/config.toml
│   ├── Cargo.toml
│   └── src/bin/main.rs
├── wroom/
│   ├── .cargo/config.toml
│   ├── Cargo.toml
│   ├── src/bin/main.rs
│   └── README.md
├── shared/
│   ├── runtime_shared.rs
│   ├── app_config.rs
│   ├── config_portal.rs
│   ├── wifi_runtime.rs
│   └── oled_display.rs
└── README.md
```

## Shared Components

Files in `shared/` represent common logic and data flow used by both platforms:

- `shared/runtime_shared.rs`
   - Main runtime flow (WiFi boot flow, AP/portal flow, periodic sensor/post loop).
- `shared/app_config.rs`
   - Persistent config storage in flash partition (SSID, password, API endpoint, API key).
- `shared/config_portal.rs`
   - Access Point HTTP portal for onboarding/config updates.
- `shared/wifi_runtime.rs`
   - WiFi state machine and reconnect behavior.
- `shared/oled_display.rs`
   - Shared SSD1306 OLED rendering helpers used by both boards.

## Board-Specific Components

Each board keeps hardware-specific setup and toolchain config in its own project.

### ESP32-C3 (`c3/`)

- Toolchain: nightly + `riscv32imc-unknown-none-elf`
- Typical pins:
   - DHT22: GPIO5
   - OLED I2C: SDA GPIO6, SCL GPIO7
   - Button: GPIO9
   - LED: GPIO8
- Flasher runner and WiFi/API env are in `c3/.cargo/config.toml`.

### ESP32-WROOM (`wroom/`)

- Toolchain: espup Xtensa environment
- Typical pins:
   - DHT22: GPIO4
   - OLED I2C: SDA GPIO21, SCL GPIO22
   - Button: GPIO0
   - LED: GPIO2
- Flasher runner and WiFi/API env are in `wroom/.cargo/config.toml`.

## Run Commands

Default features for both targets already include WiFi (`default = ["wifi"]`), so `--features wifi` is not required.

### C3 (WiFi, HTTPS)

```bash
cd esp32/c3
cargo run --release
```

### C3 (WiFi, HTTP no TLS)

```bash
cd esp32/c3
cargo run --release --features no-tls
```

### WROOM (WiFi, HTTPS)

```powershell
. "$env:USERPROFILE\export-esp.ps1"
cd esp32/wroom
cargo run --release
```

## Build Commands with Just

We are using [`just`](https://github.com/casey/just) as a command runner for consistent builds across machines.

### Install Just

```bash
# macOS
brew install just

# Windows
winget install just

# Linux
cargo install just
```

### Available Recipes

```bash
# Build
just build-wroom   # Build ESP32-WROOM (Xtensa)
just build-c3      # Build ESP32-C3 (RISC-V)
just build-all     # Build both targets

# Check (fast iteration, no full compilation)
just check-wroom   # Type-check ESP32-WROOM
just check-c3      # Type-check ESP32-C3
just check-all     # Type-check both targets

# Run/Flash
just run-wroom     # Build and flash ESP32-WROOM
just run-c3        # Build and flash ESP32-C3

# Clean
just clean-wroom   # Clean ESP32-WROOM build
just clean-c3      # Clean ESP32-C3 build
```

### Why Just?

- Same commands on any machine (macOS, Windows, Linux)
- No need to remember target flags or paths

## PowerShell Auto-Load for ESP Toolchain (Windows)

To auto-load Xtensa environment for WROOM builds:

```powershell
if (!(Test-Path $PROFILE)) {
      New-Item -ItemType File -Path $PROFILE -Force | Out-Null
}
Add-Content $PROFILE '. "$env:USERPROFILE\export-esp.ps1"'
```

Restart PowerShell (or run `. $PROFILE`).

## Why Boards Still Build Separately

A single-root workspace build with shared `include!()` was tested, but reverted for stability due to:

- dependency resolution mismatches between board targets
- architecture-specific crate behavior (RISC-V vs Xtensa)
- maintenance overhead vs current practical value

The current approach keeps deployment reliable while preserving reusable code in `shared/`.

## Troubleshooting

- `error: package ... does not contain this feature: wifi`
   - WiFi is already default. Use `cargo run --release`.
- `espflash: port not found`
   - Update board port in board-specific `.cargo/config.toml`.
- WROOM build fails with Xtensa errors
   - Load espup env first: `. "$env:USERPROFILE\export-esp.ps1"`.
