use super::{CONNECT_TIMEOUT, PROTOCOL, Tunnel, limits, parse_packet};
use crate::{
    auth,
    config::{Role, TunnelConfig},
};
use anyhow::{Context, Result, ensure};
use futures::{SinkExt, StreamExt};
use removent_core::DeviceIdentity;
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::net::UdpSocket;
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

fn handshake_request(
    config: &TunnelConfig,
    role: Role,
    identity: &DeviceIdentity,
) -> Result<tokio_tungstenite::tungstenite::http::Request<()>> {
    let audience = config.endpoint()?.uri();
    let mut request = config.websocket_url()?.into_client_request()?;
    let headers = request.headers_mut();
    if !config.token.is_empty() {
        headers.insert("authorization", format!("Bearer {}", config.token).parse()?);
    }
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs()
        .to_string();
    let nonce = hex::encode(rand::random::<[u8; 32]>());
    let proof = auth::admission_message(&audience, &config.room, role, &time, &nonce);
    headers.insert(
        "x-removent-key",
        hex::encode(identity.verifying_key().as_bytes()).parse()?,
    );
    headers.insert("x-removent-time", time.parse()?);
    headers.insert("x-removent-nonce", nonce.parse()?);
    headers.insert("x-removent-audience", audience.parse()?);
    headers.insert(
        "x-removent-signature",
        hex::encode(identity.sign(&proof).to_bytes()).parse()?,
    );
    headers.insert("x-removent-room", config.room.parse()?);
    headers.insert(
        "x-removent-role",
        match role {
            Role::Host => "host",
            Role::Client => "client",
        }
        .parse()?,
    );
    headers.insert("sec-websocket-protocol", PROTOCOL.parse()?);
    Ok(request)
}

pub async fn connect(
    config: &TunnelConfig,
    role: Role,
    identity: &DeviceIdentity,
) -> Result<Tunnel> {
    connect_with_invitation(config, role, identity, None).await
}

pub async fn connect_with_invitation(
    config: &TunnelConfig,
    role: Role,
    identity: &DeviceIdentity,
    pairing: Option<removent_core::pairing_invitation::Advertisement>,
) -> Result<Tunnel> {
    config.validate()?;
    ensure!(config.is_websocket(), "Expected a WebSocket relay profile");
    tokio::time::timeout(CONNECT_TIMEOUT, async {
        let mut request = handshake_request(config, role, identity)?;
        if let Some(ad) = &pairing {
            ensure!(
                role == Role::Host && ad.valid(),
                "Invalid invitation advertisement"
            );
            request
                .headers_mut()
                .insert("x-removent-pair", ad.locator.parse()?);
            request.headers_mut().insert(
                "x-removent-pair-exp",
                ad.expires_at_unix.to_string().parse()?,
            );
        }
        // Connect to the endpoint address; SNI only selects the TLS name.
        // HTTP Host and the signed relay audience remain bound to the endpoint.
        let roots =
            rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let mut tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth();
        if config.accept_invalid_certificate {
            tls.dangerous().set_certificate_verifier(Arc::new(
                removent_net::tls::ServerPinVerifier::new(removent_net::PinState::new([], true)),
            ));
        }
        let tcp = tokio::net::TcpStream::connect(config.endpoint()?.authority()).await?;
        tcp.set_nodelay(true)?;
        let stream = if config.insecure_loopback {
            tokio_tungstenite::MaybeTlsStream::Plain(tcp)
        } else {
            let name = rustls::pki_types::ServerName::try_from(config.tls_server_name()?)?;
            let stream = tokio_rustls::TlsConnector::from(Arc::new(tls))
                .connect(name, tcp)
                .await
                .context("Relay TLS certificate validation or handshake failed")?;
            tokio_tungstenite::MaybeTlsStream::Rustls(stream)
        };
        let (mut socket, response) =
            tokio_tungstenite::client_async_with_config(request, stream, Some(limits()))
                .await
                .map_err(|_| {
                    anyhow::anyhow!("WebSocket relay connection rejected or unavailable")
                })?;
        ensure!(
            response
                .headers()
                .get("sec-websocket-protocol")
                .and_then(|v| v.to_str().ok())
                == Some(PROTOCOL),
            "Unexpected relay protocol"
        );
        let challenge = socket.next().await.context("Missing relay challenge")??;
        let Message::Binary(challenge) = challenge else {
            anyhow::bail!("Invalid relay challenge");
        };
        let nonce: [u8; 32] = challenge
            .as_ref()
            .try_into()
            .context("Invalid relay challenge")?;
        socket
            .send(Message::Binary(
                auth::sign_challenge(identity, &config.room, role, &nonce)
                    .to_vec()
                    .into(),
            ))
            .await?;
        let ack = loop {
            match socket.next().await {
                Some(Ok(Message::Binary(ack))) => break ack,
                Some(Ok(Message::Ping(bytes))) => socket.send(Message::Pong(bytes)).await?,
                Some(Ok(Message::Pong(_))) => {}
                _ => anyhow::bail!("WebSocket relay authentication failed or host unavailable"),
            }
        };
        let invitation = role == Role::Client && config.room.starts_with("pair-");
        ensure!(
            if invitation {
                (9..=72).contains(&ack.len())
            } else {
                ack.len() == 8
            },
            "Invalid relay acknowledgement"
        );
        let resolved_room = if invitation {
            let room = std::str::from_utf8(&ack[8..])?;
            ensure!(
                crate::config::valid_room(room) && !room.starts_with("pair-"),
                "Invalid resolved room"
            );
            Some(room.to_owned())
        } else {
            None
        };
        let route = u64::from_be_bytes(ack[..8].try_into()?);
        ensure!(
            (role == Role::Host) == (route == 0),
            "Invalid relay role acknowledgement"
        );
        let mut tunnel = Tunnel::start(socket, route);
        tunnel.resolved_room = resolved_room;
        Ok::<_, anyhow::Error>(tunnel)
    })
    .await
    .context("WebSocket relay startup timed out")?
}

pub async fn start_client(
    config: &TunnelConfig,
    identity: &DeviceIdentity,
) -> Result<(
    SocketAddr,
    tokio::task::JoinHandle<Result<()>>,
    Option<String>,
)> {
    let mut tunnel = connect(config, Role::Client, identity).await?;
    let socket = UdpSocket::bind("127.0.0.1:0").await?;
    let address = socket.local_addr()?;
    let resolved_room = tunnel.resolved_room.clone();
    let task = tokio::spawn(async move {
        let sender = tunnel.sender.clone();
        let route = tunnel.route;
        let mut peer = None;
        let mut buffer = [0; crate::wire::MAX_PACKET + 1];
        loop {
            tokio::select! {
                packet = socket.recv_from(&mut buffer) => {
                    let (n, addr) = packet?;
                    if peer.is_some_and(|p| p != addr) || n == 0 || n > crate::wire::MAX_PACKET { continue; }
                    peer = Some(addr);
                    sender.packet(route, &buffer[..n]).await?;
                }
                packet = tunnel.receive() => {
                    if let Some((id, packet)) = parse_packet(&packet?) && id == route && let Some(peer) = peer {
                        socket.send_to(&packet, peer).await?;
                    }
                }
            }
        }
    });
    Ok((address, task, resolved_room))
}

struct Route {
    socket: Arc<UdpSocket>,
    last: Instant,
    reader: tokio::task::JoinHandle<()>,
}
impl Drop for Route {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

pub async fn host_loop(mut tunnel: Tunnel, target: SocketAddr) -> Result<()> {
    ensure!(
        target.ip().is_loopback() && target.port() != 0,
        "Host target must be local RVP"
    );
    let mut routes: HashMap<u64, Route> = HashMap::new();
    let mut reap = tokio::time::interval(Duration::from_secs(5));
    loop {
        tokio::select! {
            _ = reap.tick() => routes.retain(|_, r| r.last.elapsed() < Duration::from_secs(30)),
            packet = tunnel.receive() => {
                let Some((id, packet)) = parse_packet(&packet?) else { continue; };
                if id == 0 { continue; }
                if !routes.contains_key(&id) {
                    if routes.len() >= 64 { continue; }
                    let socket = Arc::new(UdpSocket::bind(if target.is_ipv4() { "127.0.0.1:0" } else { "[::1]:0" }).await?);
                    socket.connect(target).await?;
                    let reply = socket.clone();
                    let sender = tunnel.sender.clone();
                    let reader = tokio::spawn(async move {
                        let mut buffer = [0; crate::wire::MAX_PACKET + 1];
                        while let Ok(n) = reply.recv(&mut buffer).await {
                            if n != 0 && n <= crate::wire::MAX_PACKET && sender.packet(id, &buffer[..n]).await.is_err() { break; }
                        }
                    });
                    routes.insert(id, Route { socket, last: Instant::now(), reader });
                }
                let route = routes.get_mut(&id).unwrap();
                route.last = Instant::now();
                let _ = route.socket.send(&packet).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[allow(clippy::result_large_err)] // tungstenite's handshake callback fixes this error type.
    async fn websocket_sni_override_keeps_http_destination_and_certificate_validation_is_optional()
    {
        let dir = tempfile::tempdir().unwrap();
        let id = removent_core::identity::load_or_create(
            &removent_core::DataPaths {
                root: dir.path().into(),
            },
            "test",
        )
        .unwrap();
        for skip in [false, true] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let tls = rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::aws_lc_rs::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![rustls::pki_types::CertificateDer::from(id.cert_der.clone())],
                rustls::pki_types::PrivatePkcs8KeyDer::from(id.private_pkcs8_der().unwrap()).into(),
            )
            .unwrap();
            let task = tokio::spawn(async move {
                let (tcp, _) = listener.accept().await.unwrap();
                let stream = tokio_rustls::TlsAcceptor::from(Arc::new(tls))
                    .accept(tcp)
                    .await;
                let Ok(stream) = stream else {
                    assert!(!skip);
                    return;
                };
                assert_eq!(stream.get_ref().1.server_name(), Some("relay.example"));
                let mut socket = tokio_tungstenite::accept_hdr_async(stream, move |request: &tokio_tungstenite::tungstenite::handshake::server::Request, mut response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                    assert_eq!(request.headers()["host"], addr.to_string());
                    assert_eq!(request.headers()["x-removent-audience"], format!("removent://{addr}"));
                    response.headers_mut().insert("sec-websocket-protocol", PROTOCOL.parse().unwrap());
                    Ok(response)
                }).await.unwrap();
                socket
                    .send(Message::Binary(vec![0; 32].into()))
                    .await
                    .unwrap();
                assert!(matches!(
                    socket.next().await.unwrap().unwrap(),
                    Message::Binary(_)
                ));
                socket
                    .send(Message::Binary(1u64.to_be_bytes().to_vec().into()))
                    .await
                    .unwrap();
                let _ = socket.next().await;
            });
            let cfg = TunnelConfig {
                server: format!("removent://{addr}"),
                transport: removent_core::removent_uri::RelayTransport::WebSocket,
                insecure_loopback: false,
                server_fingerprint: String::new(),
                server_name: "relay.example".into(),
                accept_invalid_certificate: skip,
                host_fingerprint: String::new(),
                room: "office".into(),
                token: String::new(),
            };
            let result = connect(&cfg, Role::Client, &id).await;
            assert_eq!(result.is_ok(), skip);
            if !skip {
                assert!(format!("{:#}", result.err().unwrap()).contains("certificate"));
            }
            task.abort();
        }
    }

    #[test]
    fn admission_signature_uses_the_canonical_url_sent_to_cloudflare() {
        let dir = tempfile::tempdir().unwrap();
        let id = removent_core::identity::load_or_create(
            &removent_core::DataPaths {
                root: dir.path().to_owned(),
            },
            "device",
        )
        .unwrap();
        let config = TunnelConfig {
            server: "removent://RELAY.example:443".into(),
            transport: removent_core::removent_uri::RelayTransport::WebSocket,
            insecure_loopback: false,
            server_fingerprint: String::new(),
            server_name: String::new(),
            accept_invalid_certificate: false,
            host_fingerprint: String::new(),
            room: "office".into(),
            token: String::new(),
        };
        let request = handshake_request(&config, Role::Client, &id).unwrap();
        let h = request.headers();
        assert_eq!(h["x-removent-audience"], "removent://relay.example:443");
        assert!(!h.contains_key("authorization"));
        let message = auth::admission_message(
            h["x-removent-audience"].to_str().unwrap(),
            "office",
            Role::Client,
            h["x-removent-time"].to_str().unwrap(),
            h["x-removent-nonce"].to_str().unwrap(),
        );
        auth::verify(
            id.verifying_key().as_bytes(),
            &message,
            &hex::decode(&h["x-removent-signature"]).unwrap(),
        )
        .unwrap();
    }
}
