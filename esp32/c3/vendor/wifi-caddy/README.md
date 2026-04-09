# wifi-caddy

Platform-agnostic config storage traits, HTTP config portal, and form generation
for WiFi configuration managers.

This crate provides the core abstractions used by `wifi-caddy-proc` (derive macro)
and platform-specific runtime crates like `esp-wifi-caddy`:

- **Config storage traits** (`ConfigStorage`, `ConfigValue`, `ConfigLoadStore`,
  `ConfigGet`, `ConfigApi`, `ConfigFormGen`) for key-value config persistence.
- **HTTP config portal** (feature `portal`) with DHCP, optional captive-portal DNS,
  and a config UI server built on `edge-http` and `embassy-net`.
- **Helper types** (`ConfigStorageParams`, `ConfigUiOptions`, `ConfigHandle`) used
  by the derive macro and platform init macros.

## Usage

Add `wifi-caddy` alongside `wifi-caddy-proc` in your embedded project.
A platform crate (e.g. `esp-wifi-caddy`) provides the storage backend and
WiFi initialization.

```toml
[dependencies]
wifi-caddy = "0.1"
wifi-caddy-proc = "0.1"
# Platform-specific:
esp-wifi-caddy = "0.1"
```

## Features

| Feature        | Default | Description                                      |
|----------------|---------|--------------------------------------------------|
| `portal`       | yes     | HTTP config server, DHCP, config UI              |
| `captive`      | no      | Captive-portal DNS redirect                      |
| `debug-server` | no      | Additional HTTP server on the STA interface       |
| `defmt`        | no      | defmt logging support                            |
| `log`          | no      | log crate logging support                        |

## Server Tuning

The HTTP server's buffer sizes, timeouts, and concurrency can be overridden at
compile time via environment variables. All are optional and fall back to
sensible defaults.

| Environment Variable             | Type    | Default | Description                                    |
|----------------------------------|---------|---------|------------------------------------------------|
| `WIFI_CADDY_HANDLER_TASKS`       | `usize` | `4`     | Concurrent HTTP handler tasks                  |
| `WIFI_CADDY_TCP_BUF_SIZE`        | `usize` | `2048`  | Per-connection TCP receive/send buffer (bytes)  |
| `WIFI_CADDY_HTTP_BUF_SIZE`       | `usize` | `4096`  | HTTP request/response buffer (bytes)            |
| `WIFI_CADDY_KEEPALIVE_TIMEOUT_MS`| `u32`   | `3000`  | TCP keep-alive timeout (ms)                    |

Set them in the shell:

```sh
WIFI_CADDY_HANDLER_TASKS=6 WIFI_CADDY_HTTP_BUF_SIZE=8192 cargo build
```

Or persistently in `.cargo/config.toml` (recommended for embedded projects):

```toml
[env]
WIFI_CADDY_HANDLER_TASKS = "6"
WIFI_CADDY_TCP_BUF_SIZE = "4096"
WIFI_CADDY_HTTP_BUF_SIZE = "8192"
```
