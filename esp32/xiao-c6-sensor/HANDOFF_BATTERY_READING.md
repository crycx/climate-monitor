# Aku näidu uurimine – handoff (2026-11-19)

## Probleem
XIAO (C6-akuga, HA-s "Poiste tuba") näitab **taimeriga ärgates (300 s und)** aku
pinget ~0,1 V liiga madalalt. Pärast RESET-i ja USB küljes on näit õige.

Riistvara: D0 jagaja 2 × 1 MΩ (mõõdetud, mõlemad ~1 MΩ) + 100 nF D0–GND.
Seade XIAO flashis: `divider=2.91` (kalibreeritud akutoitel multimeetri järgi),
`interval=300`.

## Mõõtmised (akutoitel, multimeeter BAT+–GND ≈ 4,19 V kogu aeg)

| Aeg | Kuidas ärkas | HA näit |
|---|---|---|
| 16:06:48 | RESET | 4,17 V |
| 16:15:35 | taimer, 30 s und (pärast USB konsooli) | 4,16 V |
| 16:20:35 | taimer, 300 s und | 4,06 V |
| 16:25:35 | taimer, 300 s und | 4,03 V |
| 16:29:26 | RESET | 4,12 V |

## Välistatud / juba parandatud
- Raadio voolu tõttu tekkiv pingelang: aku mõõdetakse nüüd enne `Radio::start` (commit a04073e).
- Mõõtmiste hajuvus: 32 mõõtmist ja kärbitud keskmine, hajuvus ±4 mV (a04073e).
- Kondensaatori laadumine pärast ärkamist: mõõtmine on viidud DHT22 soojenemise
  lõppu, kõige varem 500 ms pärast ärkamist (145364e). See **ei aidanud**
  300 s une korral.

## Pooleli olev test
XIAO-l on salvestatud **`warmup=4000`** (tavaliselt 1500). Aku mõõdetakse siis
~3,9 s pärast käivitust, sarnaselt RESET-i juhule, kus 3 s seadistusaken
tuleb enne mõõtmist.

Vaata HA-s `sensor.climate_sensor_c6_akuga_battery_voltage` taimerinäite
alates ~16:35:
- **~4,12 V või rohkem**: ADC (või analoogosa) vajab pärast deep sleep'ist
  ärkamist paar sekundit. Püsiv parandus: initsialiseeri ADC võimalikult
  vara ja mõõda nii hilja kui võimalik, või tee esmalt rida "soojendus"-
  mõõtmisi. Vaata, kui palju see ärkveloleku aega pikendab.
- **endiselt ~4,03–4,06 V**: põhjus on mujal. Järgmised kahtlused: D0 pin'i
  olek deep sleep'is (LP IO pull-down/hold, mis jääb pärast ärkamist alles),
  kiibi temperatuur pärast pikka und (30 s une näit oli õige).

**Pärast testi pane tagasi `warmup 1500`** (config-leht "DHT22 warm-up" või
konsoolis `set warmup 1500`, `save`), muidu kulub aku kiiremini.

## Muu pooleli
- Kodune C6 gateway vajab flashi (`esp32/c6`, `cargo run --release`): Charging,
  Battery full, 100% alates 4,15 V, HA kuvatäpsus, I2C timeout.
- Kalibreerimist kontrollida ka ~3,7 V juures.
