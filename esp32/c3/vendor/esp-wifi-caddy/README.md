# esp-wifi-caddy

A `no_std` crate for ESP32 WiFi STA+AP management using Embassy and esp-radio.
Runs a station connection plus an optional access point, with a command channel
to toggle AP on/off. Optional flash-backed config persistence and a captive
HTTP portal for runtime configuration.

## How it works

```text
┌─────────────────────────────────────────────────────────────────┐
│  Your Application                                               │
│                                                                 │
│  #[derive(WifiCaddyConfig)]    ← wifi-caddy-proc                │
│  struct AppConfig { ... }           generates:                  │
│                                     • ConfigStorage impl        │
│                                     • HTML config form          │
│                                     • HTTP config API           │
│                                     • init_wifi                 │
│                                     • config change channel     │
├─────────────────────────────────────────────────────────────────┤
│  esp-wifi-caddy                                                 │
│                                                                 │
│  init(spawner, wifi)  ──→  WifiStacks + WifiCommandSender       │
│        │                        │                               │
│        ├─ STA stack             ├─ send StaUp(ssid, pass)       │
│        ├─ AP stack              ├─ send APUp(prefix)            │
│        └─ WifiRunner loop       └─ send APDown                  │
│                                                                 │
│  With feature "config":                                         │
│  ┌──────────────────────────────────────────────┐               │
│  │ Flash config storage (sequential-storage)    │               │
│  │ HTTP config UI (edge-http on AP stack)        │               │
│  │ DHCP server (edge-dhcp)                      │               │
│  │ DNS captive redirect (feature "captive")     │               │
│  └──────────────────────────────────────────────┘               │
└─────────────────────────────────────────────────────────────────┘
```

## Overview

- **Platform:** ESP32 with `esp-hal`, `esp-radio`, and `embassy-net`.
- **Configuration:** All configuration is via `WifiCaddyCommand`: send `StaUp(ssid, pass)` to
  enable STA with credentials, `APUp(prefix)` to enable the AP (full SSID = prefix + MAC), and
  `APDown` to disable the AP. The caddy starts with empty state until you send commands.
- **Optional (feature `config`, default on):** In-tree config storage traits and flash storage,
  plus captive HTTP (AP DHCP, HTTP server, config UI). No separate crates — enable or disable
  with the `config` feature. Use with **wifi-caddy-proc** to derive config structs and
  get `init_wifi`.

## Boot flow (with config + proc macro)

1. `AppConfig::init_wifi(spawner, wifi, flash, "config")` initializes WiFi, mounts flash config storage from the named partition, loads saved config, and starts the HTTP config server on the AP stack.
2. Your app receives `(WifiStacks, WifiCommandSender, ConfigHandle, config_rx)`.
3. Send `StaUp(ssid, pass)` to connect, `APUp(prefix)` to enable the AP with the config portal, `APDown` to disable it.
4. Use the returned channel receiver (`config_rx`) to react when settings are updated via the portal.

## Quick integration (without config)

1. Call `esp_wifi_caddy::init(spawner, wifi)` to get `(wifi_stacks, wifi_sender)`.
2. Store the `WifiCommandSender` where your tasks can use it.
3. Spawn a task that sends `StaUp(ssid, pass)` when you have credentials (e.g. from
   config or env). Optionally send `APUp(prefix)` to enable the AP.
4. Use `wifi_stacks.sta` and `wifi_stacks.ap` as the `embassy_net::Stack` for your
   network tasks.

## Quick integration (with config + wifi-caddy-proc)

This is the recommended path for most applications.

Your `Cargo.toml` needs these dependencies (beyond the usual `esp-hal` / `esp-rtos` /
`esp-radio` / `embassy-*` stack). The proc macro generates code that references them
directly:

```toml
[dependencies]
wifi-caddy-proc = { path = "..." }        # or git
esp-wifi-caddy  = { path = "..." }        # or git
serde           = { version = "1.0", default-features = false, features = ["derive", "alloc"] }
serde-json-core = "0.6"
esp-storage     = "0.8.1"
```

See [wifi-example/Cargo.toml](../examples/wifi-example/Cargo.toml) for a
complete working example.

```rust,ignore
use wifi_caddy_proc::WifiCaddyConfig;

#[derive(Clone, Debug, WifiCaddyConfig)]
#[config_server]
#[config_notify]
pub struct AppConfig {
    #[config_store(env_default = "WIFI_SSID", notify = "Wifi")]
    #[config_form(fieldset = "WiFi", help = "Network name")]
    wifi_ssid: String,
    #[config_store(env_default = "WIFI_PASS", notify = "Wifi")]
    #[config_form(fieldset = "WiFi", input_type = "password", help = "Password")]
    wifi_pass: String,
}
```

Then in your `main`:

```rust,ignore
let (wifi_stacks, wifi_sender, config_handle, config_rx) =
    esp_wifi_caddy::wifi_init!(AppConfig, spawner, wifi, flash, "config")
        .expect("wifi_init");
```

This single call:
1. Initializes WiFi (STA + AP stacks).
2. Mounts flash config storage from the named partition.
3. Loads saved config values.
4. Starts the HTTP config server on the AP stack (with DHCP and optional captive DNS).
5. If `#[config_notify]` is present, creates a config-update channel and returns the receiver as the 4th tuple element.

Use `config_handle.config()` to get the shared `Mutex<AppConfig>` for your tasks.
Use `config_rx.receive().await` in a task loop to react to config changes.

The config UI supports multiple tabs when you use `page = "Name"` on `#[config_form]`. Each tab
loads its data on first visit (lazy load) and shows a loading overlay until ready. Use
`#[config_ui(default_group = "Network")]` to choose which tab is active on load.

### Config page layout and CSS customization

The config page is a single HTML document with one `<style>` block: built-in CSS first, then any
`extra_css` from `#[config_ui(extra_css = "...")]`. Your CSS overrides the built-in rules when
selectors match.

**Page structure:**

```
<body>
  <div class="container">
    <header>
      <h1>   (page_heading)
      <p>    (subtitle)
    </header>
    <div class="nav">
      (nav_left)  ...  (nav_right)
    </div>
    <div class="content">
      <div class="message">           -- flash messages (save/load)
      <div class="config-tabs">       -- tab bar (only if >1 page)
        <button class="config-tab active">
      </div>
      <div class="config-tab-panel">
        <div class="config-loading-overlay">
        <form>
          <fieldset> / <legend>
          <div class="form-group"> / <label> / <input>
          <div class="button-group">  -- Reload + Save buttons
        </form>
      </div>
    </div>
  </div>
</body>
```

**CSS classes and elements you can override:**

| Selector | Purpose |
|----------|---------|
| `body` | Page background (default: gradient) |
| `.container` | Main white card |
| `header`, `header h1`, `header p` | Purple header bar |
| `.nav`, `.nav a` | Navigation bar under header |
| `.content` | Form content area |
| `fieldset`, `legend` | Field groups |
| `.form-group`, `label` | Form field wrappers |
| `input[type="text"]`, `input[type="password"]`, `input[type="number"]` | Text inputs |
| `.help-text` | Field help text |
| `.button-group` | Save/reload button row |
| `button[type="submit"]` | Save button |
| `button[type="button"]` | Reload button |
| `.config-tabs` | Tab bar container |
| `.config-tab` | Inactive tab |
| `button.config-tab.active` | Active tab (default: blue gradient) |
| `.config-tab-panel` | Tab content panel |
| `.config-loading-overlay` | Loading spinner overlay |
| `.message`, `.message.success`, `.message.error` | Flash messages |

Example:

```rust
#[config_ui(
    page_heading = "My App",
    extra_css = "body { background: #1a1a2e; } header { background: #16213e; }"
)]
```

See [wifi-caddy-proc](../wifi-caddy-proc/README.md) for all available
attributes (`#[config_store]`, `#[config_form]`, `#[config_server]`, `#[config_notify]`,
`#[config_ui]`).

## Public API

### Core (always available)

| Item | Description |
|------|-------------|
| `init(spawner, wifi)` | Initialize WiFi STA+AP; returns `(WifiStacks, WifiCommandSender)` |
| `WifiStacks` | Holds `sta: Stack` and `ap: Stack` (embassy-net stacks) |
| `WifiCommandSender` | Channel sender for `WifiCaddyCommand` |
| `WifiCaddyCommand` | `StaUp(ssid, pass)`, `APUp(prefix)`, `APDown` |
| `mk_static!` | Helper macro to create a `&'static T` from a value |

### Config feature (`config`)

| Item | Description |
|------|-------------|
| `ConfigHandle` | Shared config handle returned by `wifi_init!`; use `.config()` to get the mutex for tasks |
| `ConfigError` | Error type from `init_wifi` (e.g. backend, invalid data) |
| `config_storage::ConfigStorage` | Trait to implement an alternative storage backend |
| `config_storage::ConfigValue` | Trait to implement for custom field types in your config struct |
| `config_storage::JsSaveKind` | Enum used in `ConfigValue` impls (String, Int, Float) |
| `config_storage::MAX_VALUE_SIZE` | Max bytes per stored value (used by `ConfigStorage` default impls) |

## Features

| Feature | Default | Description |
|---------|---------|-------------|
| `defmt` | yes | defmt logging |
| `log` | no | log crate logging (mutually exclusive with `defmt`) |
| `config` | yes | In-tree config storage (traits, flash backend) and captive HTTP (DHCP, HTTP server, config UI) |
| `captive` | yes | DNS captive-portal redirect on AP; requires `config` |
| `partition-table` | yes | Resolve config partition by name from the ESP-IDF partition table |
| `nightly` | no | Enables the `impl_trait_in_assoc_type` nightly feature. **Enable this if `embassy-executor` is built with its `nightly` feature**, so that task and async code compiles correctly. |

**Feature dependencies:**

- `captive` requires `config` — the DNS redirect serves the config portal.
- `partition-table` is used by `run_inner_by_partition` to look up the flash
  partition by name. Without it, you must provide the flash range manually via
  `run_inner`.
- To minimize binary size, disable features you don't need:
  `default-features = false, features = ["config"]` gives config without captive DNS.

## Prerequisites

1. **ESP Rust toolchain** — install via [espup](https://github.com/esp-rs/espup):

   ```bash
   cargo install espup
   espup install
   ```

2. **espflash** — for flashing and monitoring:

   ```bash
   cargo install espflash
   ```

3. **Partition table** — if using flash config persistence, your partition table must include a `config` partition (type `data`, subtype `nvs` or custom). See the ESP-IDF [partition tables documentation](https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-guides/partition-tables.html).

4. **WiFi credentials** (optional): set env vars `WIFI_SSID` and `WIFI_PASS` for compile-time defaults, or configure via the captive portal at runtime.

## Examples

| Example | Description |
|---------|-------------|
| [wifi-example](../examples/wifi-example/README.md) | WiFi + config persistence + config change notifications. Best starting point for understanding the config system. |

Build and run from the example directory:

```bash
cd examples/wifi-example
cargo build-s3        # build for ESP32-S3
cargo run-s3          # flash and monitor
```

The example defines cargo aliases for multi-target builds (e.g. `cargo clippy-s3`, `cargo run-s3`). See its [README](../examples/wifi-example/README.md) for details.

## License

MIT OR Apache-2.0
