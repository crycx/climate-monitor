fn env_or(key: &str, default: &str) -> String {
    println!("cargo:rerun-if-env-changed={key}");
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn main() {
    let handler_tasks = env_or("WIFI_CADDY_HANDLER_TASKS", "4");
    let tcp_buf_size = env_or("WIFI_CADDY_TCP_BUF_SIZE", "2048");
    let http_buf_size = env_or("WIFI_CADDY_HTTP_BUF_SIZE", "4096");
    let keepalive_ms = env_or("WIFI_CADDY_KEEPALIVE_TIMEOUT_MS", "3000");

    let out = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("server_tuning.rs");

    std::fs::write(
        &out,
        format!(
            "/// Per-connection TCP receive/send buffer size in bytes.\n\
             /// Override with env var `WIFI_CADDY_TCP_BUF_SIZE` (default 2048).\n\
             const TCP_BUF_SIZE: usize = {tcp_buf_size};\n\
             \n\
             /// HTTP request/response buffer size in bytes.\n\
             /// Override with env var `WIFI_CADDY_HTTP_BUF_SIZE` (default 4096).\n\
             const HTTP_BUF_SIZE: usize = {http_buf_size};\n\
             \n\
             /// TCP keep-alive timeout in milliseconds.\n\
             /// Override with env var `WIFI_CADDY_KEEPALIVE_TIMEOUT_MS` (default 3000).\n\
             const KEEPALIVE_TIMEOUT_MS: u32 = {keepalive_ms};\n\
             \n\
             /// Number of concurrent HTTP handler tasks inside `Server::run`.\n\
             /// With `debug-server`, the debug server gets its own separate task.\n\
             /// Override with env var `WIFI_CADDY_HANDLER_TASKS` (default 4).\n\
             pub const HANDLER_TASKS: usize = {handler_tasks};\n"
        ),
    )
    .unwrap();
}
