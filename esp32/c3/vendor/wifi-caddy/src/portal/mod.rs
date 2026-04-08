//! Embassy + edge-http portal: AP DHCP, optional DNS, HTTP server loop.
//!
//! In-tree module when `config` feature is enabled.

extern crate alloc;

#[cfg(feature = "captive")]
pub(crate) mod captive;
pub mod config_group;
pub mod config_page;
pub mod config_ui;
mod dhcp;
#[cfg(feature = "captive")]
mod dns;
pub mod responses;

use core::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use edge_http::io::server::{Handler, Server};
use edge_nal::TcpBind;
use edge_nal_embassy::{Tcp, TcpBuffers};
use embassy_executor::Spawner;
use embassy_futures::select::{select, Either};
use embassy_net::Stack;

include!(concat!(env!("OUT_DIR"), "/server_tuning.rs"));

/// Run the edge-http server on the given stack with the provided handler.
///
/// Creates TCP buffers, binds to port 80, and runs the server with `HANDLER_TASKS`
/// concurrent connection handlers. Does not return under normal operation.
///
/// Connection keepalive is enforced by edge-http's `Server::run`. Per-request
/// timeouts (if desired) should be handled inside the `Handler` implementation.
pub async fn serve_loop<H: Handler>(stack: Stack<'static>, handler: H) {
    debug!("serve_loop: HANDLER_TASKS = {}", HANDLER_TASKS);
    debug!("serve_loop: TCP_BUF_SIZE = {}", TCP_BUF_SIZE);
    debug!("serve_loop: HTTP_BUF_SIZE = {}", HTTP_BUF_SIZE);
    debug!(
        "serve_loop: KEEPALIVE_TIMEOUT_MS = {}",
        KEEPALIVE_TIMEOUT_MS
    );

    loop {
        // Wait for the AP network interface to be ready before binding.
        // Without this, TCP bind succeeds structurally but packets are never delivered.
        if !stack.is_link_up() {
            info!("http: waiting for AP link...");
            stack.wait_link_up().await;
            info!("http: AP link is up");
        }

        let tcp_buffers =
            TcpBuffers::<{ HANDLER_TASKS }, { TCP_BUF_SIZE }, { TCP_BUF_SIZE }>::new();
        let tcp = Tcp::new(stack, &tcp_buffers);
        let acceptor = loop {
            match tcp
                .bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 80)))
                .await
            {
                Ok(a) => {
                    info!("http: *** SERVER READY: http://192.168.4.1/ (port 80) ***");
                    break a;
                }
                Err(_e) => {
                    error!("http: TCP bind error on port 80, retrying in 200 ms");
                    embassy_time::Timer::after(embassy_time::Duration::from_millis(200)).await;
                }
            }
        };

        let mut server = Server::<{ HANDLER_TASKS }, { HTTP_BUF_SIZE }>::new();
        match select(
            server.run(Some(KEEPALIVE_TIMEOUT_MS), acceptor, &handler),
            stack.wait_link_down(),
        )
        .await
        {
            Either::First(result) => {
                if let Err(e) = result {
                    error!("http: server error: {}", crate::fmt::DisplayFmt(&e));
                }
                // server.run() returned — exit serve_loop so run_http_config_loop
                // can restart with a fresh handler.
                info!("http: server exited, serve_loop returning");
                return;
            }
            Either::Second(_) => {
                // AP link went down (WiFi restart, APDown, or transient bounce).
                // Drop acceptor + buffers, wait for link-up, then rebind.
                info!("http: AP link went down, rebinding after link-up...");
                embassy_time::Timer::after(embassy_time::Duration::from_millis(100)).await;
                // loop back → wait_link_up → fresh bind
            }
        }
    }
}

/// Debug server: same as `serve_loop` but single concurrent handler.
#[cfg(feature = "debug-server")]
pub async fn serve_loop_debug<H: Handler>(stack: Stack<'static>, handler: H) {
    let tcp_buffers = TcpBuffers::<1, { TCP_BUF_SIZE }, { TCP_BUF_SIZE }>::new();
    let tcp = Tcp::new(stack, &tcp_buffers);

    let acceptor = match tcp
        .bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 80)))
        .await
    {
        Ok(a) => a,
        Err(_e) => {
            error!("http: TCP bind error on port 80 (debug)");
            return;
        }
    };

    let mut server = Server::<1, { HTTP_BUF_SIZE }>::new();
    if let Err(_e) = server
        .run(Some(KEEPALIVE_TIMEOUT_MS), acceptor, handler)
        .await
    {
        error!("http: debug server error");
    }
}

/// Spawns DHCP, optional DNS (if feature `captive`), then calls
/// `spawn_workers(spawner, ap_stack)` so the application can spawn its own HTTP server task.
pub fn start<F>(spawner: Spawner, ap_stack: Stack<'static>, spawn_workers: F)
where
    F: FnOnce(Spawner, Stack<'static>),
{
    spawner.spawn(dhcp::run(ap_stack)).unwrap();

    #[cfg(feature = "captive")]
    spawner.spawn(dns::run(ap_stack)).unwrap();

    spawn_workers(spawner, ap_stack);
}
