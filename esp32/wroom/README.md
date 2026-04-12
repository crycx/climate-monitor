# ESP32-WROOM Climate Sensor Firmware

Reads temperature and humidity from a DHT22 sensor and posts the data over HTTPS to the Climate Monitor backend API.

## Hardware

| Component | Detail |
|-----------|--------|
| Board | ESP32-WROOM (Xtensa ESP32) |
| Sensor | DHT22 data connected to GPIO4 (open-drain, pull-up) |
| Display | 0.91" OLED SSD1306 (I2C): SDA GPIO21, SCL/SCK GPIO22 |
| Button | BOOT button on GPIO0 (active-low) |
| LED | Built-in LED on GPIO2 (active-low on many dev boards) |

## Wiring

### DHT22

- VCC -> 3V3 (recommended)
- GND -> GND
- DATA -> GPIO4
- 10k pull-up resistor between DATA and 3V3 (if your DHT board does not include one)

### OLED 0.91" (SSD1306, I2C)

- GND -> GND
- VCC -> 3V3
- SDA -> GPIO21
- SCK (same as SCL on I2C modules) -> GPIO22

## Electrical safety note

If DHT22 is powered from 5V, do not connect DATA directly to ESP32 GPIO.
ESP32 pins are not 5V tolerant. Either power DHT22 from 3V3, or use a level shifter on DATA.

## Build and flash

On Windows, load Xtensa toolchain first:

```powershell
. "$env:USERPROFILE\export-esp.ps1"
```

Then build/flash:

```bash
cd esp32/wroom
cargo run --release
```

For plain HTTP local testing:

```bash
cd esp32/wroom
cargo run --release --features no-tls
```

## Troubleshooting

| Symptom | Likely cause | Fix |
|---------|-------------|-----|
| `espflash: port not found` | Wrong serial port in runner | Update `--port` in `wroom/.cargo/config.toml` |
| DHT22 always errors | Wiring | Check GPIO4 connection and pull-up resistor |
| OLED blank screen | Wiring/address mismatch | Check GPIO21/GPIO22, then test OLED I2C address `0x3C` vs `0x3D` |
