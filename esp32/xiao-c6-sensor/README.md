# XIAO ESP32C6 Battery Sensor (ESP-NOW)

Battery-powered DHT22 node for the **Seeed XIAO ESP32C6**. It wakes up,
measures, sends one [ESP-NOW](https://docs.espressif.com/projects/esp-idf/en/latest/esp32c6/api-reference/network/esp_now.html)
packet to the mains-powered gateway (`esp32/c6` firmware on the ESP32-C6 Super
Mini) and goes back to deep sleep. The gateway forwards the reading to Home
Assistant over MQTT (auto-discovered device) and shows it on its OLED.

No WiFi association, DHCP, TCP or TLS happens on the sensor, so the radio is
only on for a few milliseconds per cycle.

```
XIAO ESP32C6 (battery)                     ESP32-C6 Super Mini (USB power)
 wake → DHT22 → 1 ESP-NOW frame  ──────►  gateway (same radio as its WiFi STA)
 → deep sleep                               ├─ OLED page per sensor
                                            └─ MQTT → Home Assistant
```

## Wiring

| XIAO pin | GPIO | Connect to |
|---|---|---|
| D3 | GPIO21 | DHT22 **VCC** (switched by firmware, sensor is off during sleep) |
| D2 | GPIO2 | DHT22 **DATA** |
| GND | — | DHT22 GND |
| D0 / A0 | GPIO0 | Battery divider midpoint (optional, see below) |
| BAT+ / BAT− pads (bottom) | — | LiPo cell with protection circuit |

- If you use a bare DHT22, put the 10 kΩ pull-up between DATA and **D3**
  (the switched VCC), not 3V3. Most DHT22 breakout boards already do this.
- Battery divider: BAT+ → 1 MΩ → D0 → 1 MΩ → GND, plus 100 nF from D0 to GND.
  It draws ~2 µA. Without it, turn battery measurement off; readings outside
  2.5–4.5 V are reported as unknown anyway.
- The on-board RF switch (GPIO3) and antenna selector (GPIO14) are handled by
  the firmware.
- The XIAO charges the cell over USB-C. Check JST polarity before connecting
  a cell; cheap cells are often reversed.

## Settings

| Setting | Default | Meaning |
|---|---|---|
| ESP-NOW key | — | Shared secret; **must equal** `ESPNOW_KEY` in the gateway build. Nothing is sent until it is set |
| Name | empty | HA device name: "Climate Sensor (<name>)" (max 24 bytes) |
| Interval | 300 s | Sleep between measurements (10–21600 s) |
| Temp / humidity offset | 0 | Calibration offsets |
| DHT22 warm-up | 1500 ms | Settle time after powering the DHT22 |
| Battery / divider | on / 2.0 | Battery measurement on D0 |
| Antenna | internal | `external` selects U.FL |

Settings are stored in the `config` flash partition and survive reflashing
the app. Build-time values from `.cargo/config.toml` (see
`.cargo/config.toml.example`) are only the defaults.

Each sensor shows up in HA by its MAC address, so all sensors can run the same
build; give each one its own name.

### Changing settings

Setup modes start only after **power-on or RESET**: the yellow LED blinks fast
for 3 seconds.

**Over USB (computer):** connect the sensor to a computer and press RESET. It
detects the computer, stays awake and opens a console on the USB serial port.

- Easiest: open `tools/config.html` in Chrome or Edge, click **Connect**, edit,
  **Save**. **Measure & send now** checks the whole path to the gateway.
- Or any serial terminal (e.g. `espflash monitor`): type `help`. Example:

  ```
  set name Elutuba
  set interval 900
  set key my-shared-secret
  save
  test
  ```

While connected, the sensor keeps measuring and sending every interval. When
the cable is pulled it continues on battery.

**Over WiFi (phone):** press RESET, then press **BOOT** while the LED blinks.
Connect to the open WiFi `ClimateSensor-xxxxxx` and open
http://192.168.4.1/. **Save and restart** applies the settings. The portal
closes after 5 minutes without activity so it cannot drain the battery.

On a USB charger (no computer) the sensor just runs normally.

## Build & flash

Same toolchain as the other boards (nightly + `riscv32imac-unknown-none-elf`,
`espflash`), see `esp32/c6/README.md`.

```bash
cp .cargo/config.toml.example .cargo/config.toml   # once
cargo run --release
```

The runner flashes `partitions.csv`, which contains the `config` partition.
Flashing without it works, but settings cannot be saved
(`warning=no config partition`).

After flashing, the board resets with the computer attached, so it opens the
USB console directly. Expected output in normal mode:

```
DHT22: 21.4 C 45.2 %, battery 4012 mV
ESP-NOW: gateway 40:4c:ca:xx:xx:xx found on channel 6
ESP-NOW: delivered (seq 1)
Awake 1730 ms, sleeping 300 s
```

The first cycle after power-on searches channels 1–13 for the gateway. Later
cycles send one unicast frame on the remembered channel. If that fails (router
changed channel, gateway offline), the sensor searches again, last known
channel first.

To reflash a sleeping board, hold **BOOT**, tap **RESET**, release BOOT.

## Home Assistant entities

Created through MQTT Discovery by the gateway, per sensor:

- Temperature, Humidity
- Battery (%), Battery voltage (V), Signal strength (dBm), as diagnostics

Entities turn unavailable when a sensor misses ~3 reports (`expire_after`).

## Security

Packets carry a truncated HMAC-SHA256 over the sender MAC and payload with the
ESP-NOW key. The gateway drops packets with a wrong tag (logged as
`bad HMAC ... ESPNOW_KEY mismatch?`) and repeated sequence numbers. Payloads
are authenticated, not encrypted: temperature values are readable over the
air.

The WiFi portal is an open network for its 5-minute lifetime. It never shows
the stored key (the field only replaces it), but anyone nearby could change
settings while it is open.

## Power budget (estimate)

| Phase | Approx. |
|---|---|
| DHT22 warm-up + read, radio off | ~1.6 s at a few mA |
| Radio init + one unicast frame | ~0.2–0.3 s |
| Deep sleep (XIAO, DHT22 unpowered) | ~15–20 µA |

At a 5 min interval this is a few mAh per day, so a 1000 mAh cell lasts on the
order of a year. Measure your own board (e.g. PPK2) for real numbers. The
DHT22 warm-up dominates the awake time: a warm-up of 1000 ms usually works,
and an I²C sensor (SHT40) would cut it to ~10 ms. The 3 s setup window only
happens after power-on/RESET, not on timer wakes.
