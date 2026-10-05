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
    config: &config::TunnelConfig,
) -> Result<quinn::Endpoint> {
    let pin = if config.server_fingerprint.is_empty() {
        None
    } else {
        Some(config::decode_secret(&config.server_fingerprint)?)
    };
    let mut tls = tls::client_config(
        identity,
        PinState::new(pin, config.accept_invalid_certificate),
    )?;
    if pin.is_none() && !config.accept_invalid_certificate {
        let roots =
            rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let verifier = rustls::client::WebPkiServerVerifier::builder_with_provider(
            Arc::new(roots),
            Arc::new(rustls::crypto::aws_lc_rs::default_provider()),
        )
        .build()?;
        tls.dangerous().set_certificate_verifier(verifier);
    }
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

#[cfg(test)]
mod tls_tests {
    use super::*;
    use crate::config::TunnelConfig;
    use removent_core::{DataPaths, removent_uri::RelayTransport};

    #[tokio::test]
    async fn quic_verifies_by_default_and_sends_custom_sni_when_verification_is_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let identity = removent_core::identity::load_or_create(
            &DataPaths {
                root: dir.path().into(),
            },
            "test",
        )
        .unwrap();
        for (skip, pin, succeeds) in [
            (false, String::new(), false),
            (true, String::new(), true),
            (false, identity.fingerprint_hex(), true),
            (false, "aa".repeat(32), false),
        ] {
            let server = server_endpoint("127.0.0.1:0".parse().unwrap(), &identity).unwrap();
            let addr = server.local_addr().unwrap();
            let cfg = TunnelConfig {
                server: format!("removent://{addr}"),
                transport: RelayTransport::Quic,
                insecure_loopback: false,
                server_fingerprint: pin,
                server_name: "relay.example".into(),
                accept_invalid_certificate: skip,
                host_fingerprint: String::new(),
                room: "office".into(),
                token: String::new(),
            };
            cfg.validate().unwrap();
            let (name_tx, name_rx) = tokio::sync::oneshot::channel();
            let incoming = tokio::spawn(async move {
                let result = server.accept().await.unwrap().await;
                if let Ok(connection) = result {
                    let data = connection
                        .handshake_data()
                        .unwrap()
                        .downcast::<quinn::crypto::rustls::HandshakeData>()
                        .unwrap();
                    let _ = name_tx.send(data.server_name);
                    // Keep the server endpoint alive until the client has observed the handshake.
                    connection.closed().await;
                }
            });
            let client = client_endpoint(addr, &identity, &cfg).unwrap();
            let result = tokio::time::timeout(
                Duration::from_secs(3),
                client
                    .connect(addr, &cfg.tls_server_name().unwrap())
                    .unwrap(),
            )
            .await
            .unwrap();
            assert_eq!(result.is_ok(), succeeds, "skip={skip}");
            if succeeds {
                assert_eq!(
                    tokio::time::timeout(Duration::from_secs(3), name_rx)
                        .await
                        .unwrap()
                        .unwrap()
                        .as_deref(),
                    Some("relay.example")
                );
            }
            client.close(0u32.into(), b"done");
            incoming.abort();
        }
    }
}
