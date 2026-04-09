//! Captive-portal DNS server (all queries resolve to AP).

use embassy_time::Duration;
use embassy_time::Timer;

use edge_nal_embassy::{Udp, UdpBuffers};
use embassy_net::Stack;

use super::dhcp::AP_IP_ADDRESS;

/// Embassy task: run a captive-portal DNS server that resolves all queries to [`AP_IP_ADDRESS`].
#[embassy_executor::task]
pub async fn run(stack: Stack<'static>) {
    info!("dns: start DNS task");
    let buffers = UdpBuffers::<1, 1500, 1500, 2>::new();
    let mut tx_buf = [0; 1500];
    let mut rx_buf = [0; 1500];

    let udp = Udp::new(stack, &buffers);

    loop {
        if let Err(_e) = edge_captive::io::run(
            &udp,
            edge_captive::io::DEFAULT_SOCKET,
            &mut tx_buf,
            &mut rx_buf,
            AP_IP_ADDRESS,
            Duration::from_secs(60).into(),
        )
        .await
        {
            error!("dns: error");
        }
        Timer::after(Duration::from_secs(1)).await;
    }
}
