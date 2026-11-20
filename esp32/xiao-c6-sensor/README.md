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

## New sensor: first flash over USB

Every new XIAO is flashed once over USB; after that, updates can come over
the air from Home Assistant (see [Firmware update over the air](#firmware-update-over-the-air)).

1. **Tools (once per computer):** Rust nightly with `rust-src` and the
   `riscv32imac-unknown-none-elf` target, and `espflash`, as in
   `esp32/c6/README.md` (sections 1 and 2).
2. **Build config (once per computer):**

   ```bash
   cd esp32/xiao-c6-sensor
   cp .cargo/config.toml.example .cargo/config.toml
   ```

   The runner line in it must flash the OTA layout:
   `--partition-table partitions.csv --target-app-partition ota_0 --erase-parts otadata`.
3. **Flash:** connect the XIAO by USB-C and run `cargo run --release`
   (add `--port COMx` to the runner if espflash picks the wrong port). The
   log should start with `Boot: firmware <commit>`. If the board does not
   show up, hold BOOT while plugging it in.
4. **Settings, saved on the sensor:** open `esp32/tools/config.html` in
   Chrome or Edge, **Connect**, set at least the **ESP-NOW key** (same as the
   gateway's) and the **Name**, then **Save**. Or in a serial terminal:

   ```
   set key <the gateway's ESPNOW_KEY>
   set name Elutuba
   save
   ```

   They must be saved like this: firmware installed over the air is built
   without your `config.toml` values, so a key that only exists as a build
   default would be gone after the first update.
5. **Check:** `show` lists `key_set=yes` and `firmware=<commit>`; `test`
   (or **Measure & send now**) ends with `delivered=yes`. In Home
   Assistant the device "Climate Sensor (<name>)" appears with a
   **Firmware** entity.
6. Unplug, connect the battery (and the D0 divider, see Wiring), done.

The gateway (`esp32/c6` or a C3 in gateway mode) needs the OTA
`partitions.csv` too, flashed the same way once (see its README).

## Wiring

| XIAO pin | GPIO | Connect to |
|---|---|---|
| D3 | GPIO21 | DHT22 **VCC** (switched by firmware, sensor is off during sleep) |
| D2 | GPIO2 | DHT22 **DATA** |
| GND | — | DHT22 GND |
| D0 / A0 | GPIO0 | Battery divider midpoint (optional, see below) |
| D1 | GPIO1 | USB power sense for `usb_wake` (optional, see below) |
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
| USB wake | off | Wake from deep sleep when USB is plugged in (needs the D1 divider) |

Settings are stored in the `config` flash partition and survive reflashing
the app. Build-time values from `.cargo/config.toml` (see
`.cargo/config.toml.example`) are only the defaults.

Each sensor shows up in HA by its MAC address, so all sensors can run the same
build; give each one its own name.

### Changing settings

Setup modes start only after **power-on or RESET** (or plugging in USB with
`usb_wake on`, see below): the yellow LED blinks fast
for 3 seconds.

**Over USB (computer):** connect the sensor to a computer and press RESET. It
detects the computer, stays awake and opens a console on the USB serial port.

- Easiest: open `esp32/tools/config.html` (the setup page shared with the C3
  and C6 boards; it detects the sensor) in Chrome or Edge, click **Connect**,
  edit, **Save**. **Measure & send now** checks the whole path to the gateway.
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

**From Home Assistant (no cable):** the sensor's device page has
**Temperature offset**, **Humidity offset** and **Interval** (config
entities). A change is picked up by the gateway and sent with its reply to
the sensor's next reading; the sensor validates and saves it, and the HA
value updates once the sensor reports it back, so allow up to two intervals.
It follows the sensor between gateways (the command is retained in MQTT per
sensor). Needs gateways built with remote config; with an older gateway the
sensor just keeps its settings.

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

**OTA layout (since 2026-11-20):** `partitions.csv` has two app slots
(`ota_0`, `ota_1`) and `otadata` instead of one `factory` slot, for updates
over the air later. The `config` partition did not move, so settings survive.
A `config.toml` whose runner still says `--target-app-partition factory` fails
with the new table; change it to
`--target-app-partition ota_0 --erase-parts otadata` (erasing `otadata` makes
the board boot the slot just flashed). The firmware version (git commit,
`-dirty` with local changes) is printed at boot, shown by the console's
`show` and sent to Home Assistant as the device's firmware (`sw_version`).
ESP-NOW sensors report their version only to gateways built with it
(the gateway's Config reply sets `CAP_VERSION`).

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

### Wake on USB (optional)

In deep sleep the USB port is off, so plugging in a cable does nothing until
RESET. To wake on USB, feed the 5V pin to D1 through a divider and turn on
`usb_wake` (console `set usb_wake on`, config page or portal):

```
5V pin ──[47 kΩ]──┬──[68 kΩ]── GND
                  D1 (GPIO1)
```

On USB D1 sits at ~3 V; on battery the 5V pin is unpowered, so the divider
draws nothing. The wake is only armed when the sensor goes to sleep without
USB power, so on a USB charger it keeps its normal cycle. Leave `usb_wake`
off without the divider: a floating D1 would wake the board at random.

## Firmware update over the air

Once a sensor runs firmware with OTA support (flashed over USB once, see
[New sensor](#new-sensor-first-flash-over-usb)), later versions can be
installed from Home Assistant.

**From GitHub (button):** Actions → **Publish XIAO Firmware** → **Run
workflow** (pick the branch, normally `main`). It builds the sensor
firmware from that commit and publishes it to the broker; the image is also
kept as a workflow artifact. Needs repository secrets `MQTT_HOST`,
`MQTT_USER` and `MQTT_PASSWORD` (Settings → Secrets and variables →
Actions), and optionally the variables `MQTT_PORT` and `MQTT_BASE_TOPIC`.
Untick **publish** to only build and check. Then continue with step 3.

**From your computer:**

1. Commit, then build: `cargo build --release` (the version is the git
   commit; a `-dirty` build is refused, since its version does not identify
   the code).
2. Publish it to the MQTT broker the gateways use (asks for the password,
   or set `MQTT_PASSWORD`):

   ```bash
   python ../tools/publish_ota.py --host <mqtt_host> --user <mqtt_username>
   ```

3. After the sensor's next reading its **Firmware** entity in HA shows the
   update; press **Install**. With the reading after that, the gateway
   (which has downloaded and checked the image meanwhile) offers it, the
   sensor pulls it over ESP-NOW (~30 s), checks the SHA-256, switches OTA
   slot and restarts. The entity shows the new version after the next
   reading.

On battery the sensor refuses below 3.70 V. If the new version cannot be
started or comes back with the old version, the gateway offers it once more
and then gives up; press Install again to retry. There is no automatic
rollback: firmware that starts but cannot send must be flashed over USB, so
try a new version on one sensor first. A test image with its own version:
`FW_VERSION=ota-test-1 cargo build --release --target-dir target-ota`, then
`publish_ota.py --elf target-ota/riscv32imac-unknown-none-elf/release/xiao-c6-sensor`.
`publish_ota.py --clear` removes the published image.

## Home Assistant entities

Created through MQTT Discovery by the gateway, per sensor:

- Temperature, Humidity
- Battery (%), Battery voltage (V), Signal strength (dBm), as diagnostics
- Battery low (binary sensor), on below 3.60 V (~10 %): time to charge.
  Use it in an HA automation for a notification.
- Charging (binary sensor), on while the sensor runs on USB power. The
  charger then holds the cell at ~4.2 V, so Battery % is not the real level
  until USB is unplugged. Detected when a computer is attached (USB console)
  or, on a plain USB charger, with the D1 divider and `usb_wake on`. Needs a
  gateway built after this change; older gateways ignore the flag.
- Battery full (binary sensor), on after the sensor has been on USB at
  ≥ 4.15 V for an hour (the charger's constant-voltage phase is then over;
  the charge LED itself is not readable). Stays on down to 4.10 V, off when
  USB is unplugged. Tracked by the gateway in RAM, so a gateway restart
  starts the hour again.

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
