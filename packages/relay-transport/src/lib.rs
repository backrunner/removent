//! Authenticated QUIC DATAGRAM tunnels carrying opaque end-to-end RVP packets.
//! Relay credentials authorize routing only; RVP still authenticates the devices.
pub mod auth;
pub mod client;
pub mod config;
pub mod server;
pub mod websocket;
mod wire;

use anyhow::Result;
use removent_core::DeviceIdentity;
use removent_net::{PinState, tls};
use std::{net::SocketAddr, sync::Arc, time::Duration};

const ALPN: &[u8] = b"removent-relay/1";
pub const AUTH_TIMEOUT: Duration = Duration::from_secs(5);

fn transport() -> Arc<quinn::TransportConfig> {
    let mut t = quinn::TransportConfig::default();
    t.max_idle_timeout(Some(Duration::from_secs(15).try_into().unwrap()));
    t.keep_alive_interval(Some(Duration::from_secs(3)));
    t.max_concurrent_bidi_streams(1u32.into());
    t.max_concurrent_uni_streams(0u32.into());
    t.stream_receive_window(4096u32.into());
    t.receive_window(8192u32.into());
    // Bounded queues: real-time media must shed stale packets under congestion.
    // These queues sit *outside* the end-to-end QUIC priority scheduler. A
    // 512 KiB FIFO can add seconds of delay before newly prioritized input or
    // feedback reaches a slow link. Bound both directions to a short burst.
    t.datagram_receive_buffer_size(Some(32 * 1024));
    t.datagram_send_buffer_size(32 * 1024);
    t.congestion_controller_factory(Arc::new(quinn::congestion::BbrConfig::default()));
    Arc::new(t)
}

pub fn server_endpoint(addr: SocketAddr, identity: &DeviceIdentity) -> Result<quinn::Endpoint> {
    let mut tls = tls::server_config(identity, PinState::new([], true))?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let mut cfg = quinn::ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(tls)?,
    ));
    cfg.transport_config(transport());
    // Admission is bound to the source IP; migrating to a different address
    // must not bypass the source-network policy. NAT port rebinding remains valid.
    cfg.migration(false);
    Ok(quinn::Endpoint::server(cfg, addr)?)
}

fn client_endpoint(
    addr: SocketAddr,
    identity: &DeviceIdentity,
    pin: [u8; 32],
) -> Result<quinn::Endpoint> {
    let mut tls = tls::client_config(identity, PinState::new([pin], false))?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let mut cfg = quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(tls)?,
    ));
    cfg.transport_config(transport());
    let mut ep = quinn::Endpoint::client(
        if addr.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        }
        .parse()?,
    )?;
    ep.set_default_client_config(cfg);
    Ok(ep)
}
