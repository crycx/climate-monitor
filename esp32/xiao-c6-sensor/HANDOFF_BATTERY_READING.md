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

### Tulemused (kodus, pärast transporti)
16:35–16:50 oli HA-s üksainus näit 4,22 V (25,8 °C), siis 16:50–17:45 andmed
puudusid (sensor oli teel). Kodus, akutoitel, taimeriga ärgates:

| Aeg | HA näit | Temp | Multimeeter |
|---|---|---|---|
| 17:47:49 | 3,99 V | 20,0 °C | – |
| 17:52:53 | 4,03 V | 22,3 °C | – |
| 17:57:57 | 4,04 V | 23,0 °C | – |
| 18:03–20:05 | 4,05 V püsivalt | 23,5–23,9 °C | BAT+–GND 4,11 V; D0–GND 1,68 V |

Pärast soojenemist näitab HA ~0,06 V vähem kui multimeeter (enne testi oli
vahe 0,13–0,16 V). Kas `warmup=4000` oli selle ajal veel peal, pole kinnitatud.
Pinge tõus 3,99 → 4,05 V käis koos soojenemisega 20 → 23,5 °C.

Pärast aku uuesti ühendamist (külmkäivitus, 20:05) näitas HA 4,15 V, järgmised
taimerinäidud 4,09 V (26,1 °C) ja 4,07 V (~25 °C); multimeeter BAT+ 4,11 V.

### ADC vs multimeeter (USB küljes, 20:45, ADC soojendusega püsivara)
- Sensor: `battery_mv=4312` → ADC näeb D0 peal 4312 / 2,91 ≈ 1,48 V.
  Logi: `first conversion 4312 mV, after warm-up 4312 mV` (USB toitel triivi pole).
- Multimeeter: BAT+ 4,17 V, D0 1,70 V.
- Jagaja suhe multimeetriga on mõlemal mõõtmisel sama: 4,11/1,68 ≈ 4,17/1,70 ≈ 2,45.
- ADC näeb D0 pinget vähemalt 0,22 V (~13 %) madalamana kui multimeeter
  (multimeeter ise saab näitu ainult langetada). USB toitel näitab sensor
  0,14 V üle, akult taimeriga ärgates ~0,06 V alla: muutuv osa on ADC poolel.
- Fluksijääk on ainult BAT+ joodiskohal; D0 sõlm on õhus, seal leket pole näha.

`warmup` pandi tagasi 1500 peale ja salvestati (20:47).

### ADC soojenduse test (pidevad mõõtmised ärkamisest kuni aku mõõtmiseni)
- 21:01:23 RESET: HA 4,18 V. 21:06:23 taimer (300 s): HA 4,07 V, 25,4 °C.
  Multimeeter BAT+ 4,11 V. Taimeriga ärgates endiselt ~0,04 V alla, RESET-i ja
  taimeri vahe ~0,11 V. **Ei aidanud**, muudatus võeti koodist tagasi
  (XIAO-s on see veel peal kuni järgmise flashini; kahjutu).
- Eraldi viga: pärast USB konsooli lahtiühendamist (~20:52) ei saatnud sensor
  ~10 min midagi, kuni tehti RESET. Taimeriga ärkamine ise töötab (21:06).
  Kahtlus: konsoolist väljumine (`console.rs`, HOST_GONE_MS -> deep_sleep)
  või `println!` blokeerub, kui USB host on kadunud. Uurimata.

### Konsooli `adc` diagnostika (USB küljes, 21:17–21:20)
- eFuse kalibreerimine on olemas (`efuse_cal_version=1`, init 2166, ref 1425 @ 1370 mV),
  seega kalibreerimist ei tehta käivitusel uuesti; see ei seleta RESET/taimeri vahet.
- `gnd_code_now=2158` mõlemal korral (eFuse 2166): nullpunkt praegu stabiilne.
  Külmalt ärgates pole seda mõõdetud (USB pealt ei saa).
- Line ja curve annavad sama (D0 ≈ 1,46–1,50 V); skeemi vahetus ei aita.
- `ref_code_now` on 1–4: referentsallikas C6 peal ilmselt ei tööta, ignoreeri.
- Multimeeter D0 peal näitas 1,68 / 1,70 / 1,04 V: kõrge takistusega sõlm,
  multimeetri D0 näit pole usaldusväärne. Võrdle ainult BAT+.
- USB toitel (laadimine) näitab sensor 0,15–0,3 V üle; ebaoluline, HA näitab
  siis Charging.

### Lahendus
`divider` = **2,94** (salvestatud 21:17), kalibreeritud taimeriga ärkamiste järgi.
21:25:58 taimer: HA 4,14 V, multimeeter 4,12 V. RESET-i näit jääb ~0,1 V üle,
temperatuuri mõju ~0,015 V/°C jääb alles.

**Pärast testi pane tagasi `warmup 1500`** (tehtud 20:47) (config-leht "DHT22 warm-up" või
konsoolis `set warmup 1500`, `save`), muidu kulub aku kiiremini.

## Muu pooleli
- ~~Kodune C6 gateway vajab flashi~~: tehtud 2026-11-19 17:51 (Nextion välja lülitatud).
- USB lahtiühendamise järel ~10 min vahe näitudes: sensor magas täis intervalli
  lahtiühendamisest. Parandatud: magab ainult järgmise ette nähtud saatmiseni
  (kontrollitud 21:25:58).
- Kalibreerimist kontrollida ka ~3,7 V juures.
