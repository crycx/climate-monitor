# AP Portal Debug Handoff (2026-04-08)

## Eesmärk
Lahendada olukord, kus AP portal aadressil 192.168.4.1 avaneb korraks ja seejärel brauser näitab `ERR_CONNECTION_REFUSED`.

## Muudetud failid
- `esp32/c3/src/bin/main.rs`
- `esp32/c3/vendor/esp-wifi-caddy/src/lib.rs`
- `esp32/c3/vendor/wifi-caddy/src/portal/mod.rs`
- `esp32/c3/vendor/wifi-caddy/src/run_http.rs`
- `esp32/c3/vendor/wifi-caddy/src/portal/dhcp.rs`

## Tehtud muudatused

### 1) Boot flow ümber tehtud (STA enne, AP fallback pärast)
Fail: `esp32/c3/src/bin/main.rs`
- Bootis tehakse kõigepealt STA connect katse (kuni 15 s).
- Kui STA ei saa üles, siis avatakse AP portal (`192.168.4.1`).
- Kui STA ühendub, AP-d ei avata.

Olulised logid:
- `Boot: connecting to ...`
- `Boot: waiting up to 15 s for STA connection...`
- timeout korral: `Boot: STA connect timeout, starting AP portal at 192.168.4.1`

### 2) Lisatud STA väljalülitus AP recovery/missing-config puhul
Failid:
- `esp32/c3/vendor/esp-wifi-caddy/src/lib.rs`
- `esp32/c3/src/bin/main.rs`

Muudatus:
- Lisatud uus käsk `WifiCaddyCommand::StaDown`.
- AP avamisel fallback/recovery/missing-config teedes saadetakse enne `StaDown`.

Mõte:
- Vältida olukorda, kus STA reconnect loop segab AP/HTTP stabiilsust.

### 3) Vältitud tarbetu WiFi restart sama mode puhul
Fail: `esp32/c3/vendor/esp-wifi-caddy/src/lib.rs`
- `ensure_wifi_started_with_config` ei tee enam stop/start tsüklit, kui mode-variant ei muutu (nt ApSta -> ApSta).
- Lisatud `started_mode` jälgimine.

### 4) HTTP serveri taastumise viited lühendatud
Failid:
- `esp32/c3/vendor/wifi-caddy/src/run_http.rs`
- `esp32/c3/vendor/wifi-caddy/src/portal/mod.rs`

Muudatus:
- `serve_loop` restart viive: 500 ms -> 50 ms.
- TCP bind retry viive: 2 s -> 200 ms.

Mõte:
- Vähendada `connection refused` akna pikkust, kui server tsükkel katkeb.

### 5) AP perioodilised scan-id runtime-lipuga välja lülitatud
Fail: `esp32/c3/vendor/esp-wifi-caddy/src/lib.rs`
- `scan_refresh_enabled` lisatud ja vaikimisi `false`.
- `refresh_scan_results()` tagastab kohe, kui lipp on false.

Mõte:
- Scan võis katkestada HTTP acceptori ja tekitada brauseris refused tsükli.

### 6) AP/DHCP käivitus
Failid:
- `esp32/c3/vendor/wifi-caddy/src/portal/mod.rs`
- `esp32/c3/vendor/wifi-caddy/src/portal/dhcp.rs`

Muudatus:
- Eemaldatud varasem `wait_link_up()` enne bind katseid, et server/protokollid prooviksid kohe bindida.

## Build staatus
- `cargo check` (folderis `esp32/c3`) läheb läbi.
- `cargo run --release` flashib edukalt.

## Viimane teadaolev runtime log
Esines kaks erinevat testi:
1. STA timeout test:
- `Boot: connecting to ASUS_B0_2G`
- `Boot: waiting up to 15 s for STA connection...`
- `Boot: STA connect timeout, starting AP portal at 192.168.4.1`

2. STA edukas test:
- `WiFi connected during boot.`
- `Got IP: Cidr { address: 192.168.50.145, prefix_len: 24 }`
- `POST -> HTTP 201`

## Homme jätkamise testiplaan
1. Tee kontrollitud AP fallback test:
- Lülita ruuter välja või pane teadlikult vale SSID.
- Taaskäivita seade.
- Oota timeout logini: `Boot: STA connect timeout...`.
- Ühendu AP-ga ja ava `http://192.168.4.1/`.

2. Kui probleem püsib (`ERR_CONNECTION_REFUSED`):
- Salvesta serial monitorist 20-40 rida alates timeout logist.
- Salvesta brauseri Network + Console screenshot.
- Pane kirja kas telefon sai AP DHCP IP aadressi (nt 192.168.4.x).

3. Kontrolli AP kliendi IP:
- Kui klient ei saa DHCP IP-d, fokus järgmine samm DHCP pathile.
- Kui klient saab IP, fokus järgmine samm HTTP bind/acceptor telemetryle.

## Kiirkäsklused
- Build check:
  - `Set-Location e:/Arendused/climate_monitor/esp32/c3; cargo check`
- Flash + monitor:
  - `Set-Location e:/Arendused/climate_monitor/esp32/c3; cargo run --release`

## Märkus
See fail on mõeldud cross-device handoff-iks, et teises arvutis saaks jätkata ilma eelmise chatita.
