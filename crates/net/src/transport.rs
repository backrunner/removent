//! QUIC endpoint construction (quinn + the TLS config above) and transport parameters.

use crate::error::{NetError, Result};
use crate::tls::{PinState, client_config, server_config};
use removent_core::DeviceIdentity;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

/// QUIC parameters (protocol.md §7.2): idle 10s / keep-alive 2s.
fn transport_config() -> Result<quinn::TransportConfig> {
    let mut tc = quinn::TransportConfig::default();
    let idle = quinn::IdleTimeout::try_from(Duration::from_secs(10))
        .map_err(|e| NetError::Endpoint(e.to_string()))?;
    tc.max_idle_timeout(Some(idle));
    tc.keep_alive_interval(Some(Duration::from_secs(2)));
    Ok(tc)
}

pub fn make_client_endpoint(
    bind_addr: SocketAddr,
    identity: &DeviceIdentity,
    pin: PinState,
) -> Result<(quinn::Endpoint, PinState)> {
    let mut ep =
        quinn::Endpoint::client(bind_addr).map_err(|e| NetError::Endpoint(e.to_string()))?;
    let qcc =
        quinn::crypto::rustls::QuicClientConfig::try_from(client_config(identity, pin.clone())?)
            .map_err(|e| NetError::Tls(e.to_string()))?;
    let mut cfg = quinn::ClientConfig::new(Arc::new(qcc));
    cfg.transport_config(Arc::new(transport_config()?));
    ep.set_default_client_config(cfg);
    Ok((ep, pin))
}

pub fn make_server_endpoint(
    listen_addr: SocketAddr,
    identity: &DeviceIdentity,
    pin: PinState,
) -> Result<(quinn::Endpoint, PinState)> {
    let mut sc = quinn::ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(server_config(identity, pin.clone())?)
            .map_err(|e| NetError::Tls(e.to_string()))?,
    ));
    sc.transport_config(Arc::new(transport_config()?));
    let ep =
        quinn::Endpoint::server(sc, listen_addr).map_err(|e| NetError::Endpoint(e.to_string()))?;
    Ok((ep, pin))
}
