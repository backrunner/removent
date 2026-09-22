use anyhow::Result;
use futures::{SinkExt, StreamExt};
use removent_core::removent_uri::RelayTransport;
use removent_core::{DataPaths, DeviceIdentity, identity};
use removent_net::{PinState, RvpConnection, make_client_endpoint, make_server_endpoint};
use removent_proto::{Caps, ControlMsg, HandshakeClient, HandshakeServer, Hello};
use removent_relay::{
    client::{self, ClientBridge},
    config::{Role, Room, ServerConfig, TunnelConfig, token_hash},
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

struct Fixture {
    _dir: tempfile::TempDir,
    host: DeviceIdentity,
    viewer: DeviceIdentity,
    host_cfg: TunnelConfig,
    client_cfg: TunnelConfig,
    stop: CancellationToken,
    server: tokio::task::JoinHandle<Result<()>>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.cancel();
        self.server.abort();
    }
}
impl Fixture {
    async fn new() -> Self {
        Self::transport(false, None).await
    }
    async fn transport(websocket: bool, idle: Option<Duration>) -> Self {
        Self::secure_transport(websocket, idle, 0).await
    }
    async fn secure_transport(websocket: bool, idle: Option<Duration>, auth_mode: u8) -> Self {
        Self::with_cidrs(websocket, idle, auth_mode, vec![]).await
    }
    async fn with_cidrs(
        websocket: bool,
        idle: Option<Duration>,
        auth_mode: u8,
        allowed_cidrs: Vec<ipnet::IpNet>,
    ) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let identity = |name: &str| {
            identity::load_or_create(
                &DataPaths {
                    root: dir.path().join(name),
                },
                name,
            )
            .unwrap()
        };
        let relay = identity("relay");
        let host = identity("host");
        let viewer = identity("viewer");
        let ep = removent_relay::server_endpoint("127.0.0.1:0".parse().unwrap(), &relay).unwrap();
        let listener = if websocket {
            Some(tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap())
        } else {
            None
        };
        let address = listener
            .as_ref()
            .map(|s| s.local_addr().unwrap())
            .unwrap_or(ep.local_addr().unwrap());
        let host_token = if auth_mode == 1 {
            String::new()
        } else {
            "11".repeat(32)
        };
        let client_token = if auth_mode == 1 {
            String::new()
        } else {
            "22".repeat(32)
        };
        let host_cfg = TunnelConfig {
            server: format!("removent://{address}"),
            transport: if websocket {
                RelayTransport::WebSocket
            } else {
                RelayTransport::Quic
            },
            insecure_loopback: websocket,
            server_fingerprint: if websocket {
                String::new()
            } else {
                relay.fingerprint_hex()
            },
            host_fingerprint: host.fingerprint_hex(),
            room: "office".into(),
            token: host_token.clone(),
        };
        let client_cfg = TunnelConfig {
            token: client_token.clone(),
            ..host_cfg.clone()
        };
        let config = ServerConfig {
            listen: address,
            allowed_cidrs,
            identity_dir: dir.path().join("relay"),
            max_connections: 64,
            max_clients_per_room: 4,
            max_bytes_per_second: 1_000_000_000,
            rooms: vec![Room {
                name: "office".into(),
                host_token_sha256: if host_token.is_empty() {
                    String::new()
                } else {
                    hex::encode(token_hash(&host_token).unwrap())
                },
                client_token_sha256: if client_token.is_empty() {
                    String::new()
                } else {
                    hex::encode(token_hash(&client_token).unwrap())
                },
                host_public_keys: if auth_mode > 0 {
                    vec![hex::encode(host.verifying_key().as_bytes())]
                } else {
                    vec![]
                },
                client_public_keys: if auth_mode > 0 {
                    vec![hex::encode(viewer.verifying_key().as_bytes())]
                } else {
                    vec![]
                },
            }],
        };
        let stop = CancellationToken::new();
        let server = match listener {
            Some(listener) => tokio::spawn(removent_relay::websocket::serve(
                listener,
                config,
                stop.clone(),
                idle,
            )),
            None => tokio::spawn(removent_relay::server::serve(ep, config, stop.clone())),
        };
        Self {
            _dir: dir,
            host,
            viewer,
            host_cfg,
            client_cfg,
            stop,
            server,
        }
    }
}

#[tokio::test]
async fn authentication_pinning_roles_capacity_and_host_recovery() {
    let f = Fixture::new().await;
    assert!(
        client::connect(&f.client_cfg, Role::Client, &f.viewer)
            .await
            .is_err(),
        "offline host"
    );
    assert!(
        client::connect(&f.client_cfg, Role::Host, &f.viewer)
            .await
            .is_err(),
        "client token cannot register host"
    );
    let mut wrong = f.host_cfg.clone();
    wrong.server_fingerprint = "ff".repeat(32);
    assert!(
        client::connect(&wrong, Role::Host, &f.host).await.is_err(),
        "no TOFU for relay"
    );
    wrong = f.host_cfg.clone();
    wrong.token = "00".repeat(32);
    assert!(client::connect(&wrong, Role::Host, &f.host).await.is_err());
    let host = client::connect(&f.host_cfg, Role::Host, &f.host)
        .await
        .unwrap();
    assert!(
        client::connect(&f.host_cfg, Role::Host, &f.host)
            .await
            .is_err(),
        "duplicate host rejected"
    );
    wrong = f.client_cfg.clone();
    wrong.room = "another-room".into();
    assert!(
        client::connect(&wrong, Role::Client, &f.viewer)
            .await
            .is_err()
    );
    let mut clients = Vec::new();
    for _ in 0..4 {
        clients.push(
            client::connect(&f.client_cfg, Role::Client, &f.viewer)
                .await
                .unwrap(),
        );
    }
    assert!(
        client::connect(&f.client_cfg, Role::Client, &f.viewer)
            .await
            .is_err(),
        "bounded room"
    );
    drop(host);
    tokio::time::timeout(Duration::from_secs(2), clients[0].connection.closed())
        .await
        .unwrap();
    let _reconnected = client::connect(&f.host_cfg, Role::Host, &f.host)
        .await
        .unwrap();
}

#[tokio::test]
async fn controller_cannot_inject_another_controllers_route() {
    let f = Fixture::new().await;
    let _host = client::connect(&f.host_cfg, Role::Host, &f.host)
        .await
        .unwrap();
    let attacker = client::connect(&f.client_cfg, Role::Client, &f.viewer)
        .await
        .unwrap();
    let victim = client::connect(&f.client_cfg, Role::Client, &f.viewer)
        .await
        .unwrap();
    let mut frame = victim.route.to_be_bytes().to_vec();
    frame.extend_from_slice(&[0; 9]);
    attacker.connection.send_datagram(frame.into()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), attacker.connection.closed())
        .await
        .unwrap();
    assert!(victim.connection.close_reason().is_none());
}

struct HostTask(tokio::task::JoinHandle<Result<()>>);

#[tokio::test]
async fn container_readiness_probes_get_http_responses_without_websocket_auth() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let f = Fixture::transport(true, None).await;
    let address = f.host_cfg.endpoint().unwrap().authority();
    for path in ["/", "/healthz"] {
        let mut socket = tokio::net::TcpStream::connect(&address).await.unwrap();
        socket.write_all(format!("GET {path} HTTP/1.1\r\nHost: containerstarthealthcheck\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        let mut reply = String::new();
        tokio::time::timeout(Duration::from_secs(2), socket.read_to_string(&mut reply))
            .await
            .unwrap()
            .unwrap();
        assert!(reply.starts_with("HTTP/1.1 200 OK"), "{reply}");
        assert!(reply.ends_with("ok"));
    }
}

#[tokio::test]
async fn websocket_credentials_roles_capacity_route_isolation_and_cleanup() {
    use removent_relay::websocket::connect;
    let f = Fixture::transport(true, None).await;
    assert!(connect(&f.client_cfg, Role::Host, &f.host).await.is_err());
    let mut bad = f.host_cfg.clone();
    bad.token = "00".repeat(32);
    assert!(connect(&bad, Role::Host, &f.host).await.is_err());
    bad = f.host_cfg.clone();
    bad.room = "other".into();
    assert!(connect(&bad, Role::Host, &f.host).await.is_err());
    let mut host = connect(&f.host_cfg, Role::Host, &f.host).await.unwrap();
    assert!(connect(&f.host_cfg, Role::Host, &f.host).await.is_err());
    let mut attacker = connect(&f.client_cfg, Role::Client, &f.viewer)
        .await
        .unwrap();
    let mut victim = connect(&f.client_cfg, Role::Client, &f.viewer)
        .await
        .unwrap();
    let extra1 = connect(&f.client_cfg, Role::Client, &f.viewer)
        .await
        .unwrap();
    let extra2 = connect(&f.client_cfg, Role::Client, &f.viewer)
        .await
        .unwrap();
    assert!(
        connect(&f.client_cfg, Role::Client, &f.viewer)
            .await
            .is_err()
    );
    attacker.send_packet(victim.route, b"forged").await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), attacker.receive())
            .await
            .unwrap()
            .is_err()
    );
    victim.send_packet(victim.route, b"alive").await.unwrap();
    let packet = tokio::time::timeout(Duration::from_secs(2), host.receive())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&packet[8..], b"alive");
    drop(host);
    assert!(
        tokio::time::timeout(Duration::from_secs(2), victim.receive())
            .await
            .unwrap()
            .is_err()
    );
    drop((extra1, extra2));
    let _host = connect(&f.host_cfg, Role::Host, &f.host).await.unwrap();
}

#[tokio::test]
async fn websocket_cold_start_waits_for_daemon_and_idle_ignores_host_presence() {
    use removent_relay::websocket::connect;
    let mut f = Fixture::transport(true, Some(Duration::from_secs(1))).await;
    let config = f.client_cfg.clone();
    let viewer = f.viewer.clone();
    let controller = tokio::spawn(async move { connect(&config, Role::Client, &viewer).await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let host = connect(&f.host_cfg, Role::Host, &f.host).await.unwrap();
    let controller = controller.await.unwrap().unwrap();
    tokio::time::sleep(Duration::from_millis(2200)).await;
    assert!(
        !f.server.is_finished(),
        "an active controller prevents idle shutdown"
    );
    drop(controller);
    tokio::time::timeout(Duration::from_secs(4), &mut f.server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    drop(host);
}

#[tokio::test]
async fn websocket_rejects_text_and_oversized_frames() {
    use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};
    let f = Fixture::transport(true, None).await;
    let _host = removent_relay::websocket::connect(&f.host_cfg, Role::Host, &f.host)
        .await
        .unwrap();
    for invalid in [
        Message::Text("invalid".into()),
        Message::Binary(vec![0; 8192].into()),
    ] {
        let mut request = f
            .client_cfg
            .websocket_url()
            .unwrap()
            .into_client_request()
            .unwrap();
        let headers = request.headers_mut();
        headers.insert(
            "authorization",
            format!("Bearer {}", f.client_cfg.token).parse().unwrap(),
        );
        headers.insert(
            "x-removent-key",
            hex::encode(f.viewer.verifying_key().as_bytes())
                .parse()
                .unwrap(),
        );
        headers.insert("x-removent-room", "office".parse().unwrap());
        headers.insert("x-removent-role", "client".parse().unwrap());
        headers.insert(
            "sec-websocket-protocol",
            removent_relay::websocket::PROTOCOL.parse().unwrap(),
        );
        let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
        let Message::Binary(challenge) = socket.next().await.unwrap().unwrap() else {
            panic!("challenge");
        };
        let proof = removent_relay::auth::sign_challenge(
            &f.viewer,
            "office",
            Role::Client,
            &challenge.as_ref().try_into().unwrap(),
        );
        socket
            .send(Message::Binary(proof.to_vec().into()))
            .await
            .unwrap();
        let _ack = socket.next().await.unwrap().unwrap();
        socket.send(invalid).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(Ok(message)) = socket.next().await {
                if matches!(message, Message::Close(_)) {
                    break;
                }
            }
        })
        .await
        .expect("bad frames must close promptly");
    }
}

#[tokio::test]
async fn wss_rejects_untrusted_tls_and_profiles_cannot_disable_verification() {
    let f = Fixture::transport(true, None).await;
    let mut cfg = f.host_cfg.clone();
    cfg.insecure_loopback = false;
    for address in [
        "ws://example.com/v1/tunnel",
        "wss://user:token@example.com/v1/tunnel",
        "wss://example.com/v1/tunnel?token=secret",
        "wss://example.com/wrong",
    ] {
        cfg.server = address.into();
        assert!(cfg.validate().is_err());
    }
    cfg.server = "removent://example.com:443".into();
    cfg.server_fingerprint = "11".repeat(32);
    assert!(
        cfg.validate().is_err(),
        "a QUIC pin must not be silently ignored"
    );
    let tls = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![f.host.cert_der.clone().into()],
        rustls::pki_types::PrivatePkcs8KeyDer::from(f.host.private_pkcs8_der().unwrap()).into(),
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    cfg.server = format!("removent://{}", listener.local_addr().unwrap());
    cfg.insecure_loopback = false;
    cfg.server_fingerprint.clear();
    let server = tokio::spawn(async move {
        let socket = listener.accept().await.unwrap().0;
        assert!(
            tokio_rustls::TlsAcceptor::from(Arc::new(tls))
                .accept(socket)
                .await
                .is_err()
        );
    });
    assert!(
        removent_relay::websocket::connect(&cfg, Role::Host, &f.host)
            .await
            .is_err()
    );
    server.await.unwrap();
}
impl Drop for HostTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn rvp_pair(
    f: &Fixture,
) -> (
    RvpConnection,
    RvpConnection,
    ClientBridge,
    HostTask,
    [quinn::Endpoint; 2],
) {
    let (host_ep, _) = make_server_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &f.host,
        PinState::new([f.viewer.fingerprint], false),
    )
    .unwrap();
    let tunnel = client::connect_host(&f.host_cfg, &f.host).await.unwrap();
    let host_task = HostTask(tokio::spawn(tunnel.run(host_ep.local_addr().unwrap())));
    let bridge = ClientBridge::start(&f.client_cfg, &f.viewer).await.unwrap();
    let (client_ep, _) = make_client_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &f.viewer,
        PinState::new([f.host.fingerprint], false),
    )
    .unwrap();
    let (viewer, host) = tokio::join!(
        async {
            client_ep
                .connect(bridge.address, "removent")
                .unwrap()
                .await
                .unwrap()
        },
        async { host_ep.accept().await.unwrap().await.unwrap() }
    );
    (
        RvpConnection::new(viewer),
        RvpConnection::new(host),
        bridge,
        host_task,
        [client_ep, host_ep],
    )
}

#[tokio::test]
async fn real_rvp_end_to_end_identity_handshake_control_and_fragmented_media() {
    for websocket in [false, true] {
        tokio::time::timeout(Duration::from_secs(20), async {
        let f = Fixture::transport(websocket, None).await;
        let (viewer, host, _bridge, _host_task, _endpoints) = rvp_pair(&f).await;
        assert_eq!(viewer.peer_fingerprint(), Some(f.host.fingerprint));
        assert_eq!(host.peer_fingerprint(), Some(f.viewer.fingerprint));
        let (client, server) = tokio::join!(
            viewer.connect_handshake(HandshakeClient {
                magic: removent_proto::MAGIC, proto_version: removent_proto::PROTO_VERSION, feature_bits: 0,
                hello: Hello { app_version: "test".into(), device_name: "viewer".into(), os_version: "test".into(), caps: Caps::all(), resume_token: None }
            }),
            host.accept_handshake(|_| HandshakeServer { proto_version: removent_proto::PROTO_VERSION, feature_bits: 0, device_name: "host".into(), resume_accepted: None, peer_known: true })
        );
        let (_, mut tx, _) = client.unwrap();
        let (_, _, mut rx) = server.unwrap();
        tx.send(ControlMsg::Ping { ts_us: 42 }).await.unwrap();
        assert!(matches!(rx.next().await.unwrap().unwrap(), removent_net::ControlItem::Msg(msg) if *msg == ControlMsg::Ping { ts_us: 42 }));
        let payload = Arc::new((0..1_048_576).map(|i| (i % 251) as u8).collect::<Vec<_>>());
        let expected = payload.clone();
        let ((), actual) = tokio::join!(async {
            let mut stream = host.open_media_stream().await.unwrap();
            stream.write_all(&payload).await.unwrap(); stream.finish().unwrap();
            stream.stopped().await.unwrap();
        }, async {
            viewer.accept_media_stream().await.unwrap().read_to_end(2_000_000).await.unwrap()
        });
        assert_eq!(actual, *expected);
    }).await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "release-mode loopback performance measurement"]
async fn relay_rvp_benchmark() {
    let f = Fixture::new().await;
    benchmark(&f, "QUIC").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "release-mode loopback performance measurement"]
async fn websocket_rvp_benchmark() {
    let f = Fixture::transport(true, None).await;
    benchmark(&f, "WebSocket").await;
}

async fn benchmark(f: &Fixture, transport: &str) {
    let (viewer, host, _bridge, _host_task, _endpoints) = rvp_pair(f).await;
    let block = vec![0x5a; 64 * 1024];
    let total = 64 * 1024 * 1024;
    let start = Instant::now();
    let ((), bytes) = tokio::join!(
        async {
            let mut stream = host.open_media_stream().await.unwrap();
            for _ in 0..total / block.len() {
                stream.write_all(&block).await.unwrap();
            }
            stream.finish().unwrap();
            stream.stopped().await.unwrap();
        },
        async {
            let mut stream = viewer.accept_media_stream().await.unwrap();
            let mut buffer = vec![0; 64 * 1024];
            let mut bytes = 0;
            while let Some(n) = stream.read(&mut buffer).await.unwrap() {
                bytes += n;
            }
            bytes
        }
    );
    assert_eq!(bytes, total);
    println!(
        "RVP via {transport} relay loopback: bytes={bytes} seconds={:.3} Mbps={:.2} build={}",
        start.elapsed().as_secs_f64(),
        bytes as f64 * 8. / start.elapsed().as_secs_f64() / 1e6,
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    let mut samples = Vec::new();
    for _ in 0..200 {
        let start = Instant::now();
        let (mut send, mut recv) = viewer.inner().open_bi().await.unwrap();
        let echo = async {
            let (mut send, mut recv) = host.inner().accept_bi().await.unwrap();
            let mut data = [0; 8];
            recv.read_exact(&mut data).await.unwrap();
            send.write_all(&data).await.unwrap();
        };
        let request = async {
            send.write_all(b"12345678").await.unwrap();
            let mut data = [0; 8];
            recv.read_exact(&mut data).await.unwrap();
        };
        tokio::join!(echo, request);
        samples.push(start.elapsed().as_secs_f64() * 1000.);
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "Relay stream RTT ms: p50={:.3} p95={:.3} p99={:.3}",
        samples[100], samples[190], samples[198]
    );
}

#[tokio::test]
async fn both_transports_enforce_registered_devices_with_and_without_credentials() {
    for websocket in [false, true] {
        for mode in [1, 2] {
            let f = Fixture::secure_transport(websocket, None, mode).await;
            assert!(
                client::connect_host(&f.host_cfg, &f.viewer).await.is_err(),
                "controller cannot register as host even with a host credential"
            );
            let _host = client::connect_host(&f.host_cfg, &f.host).await.unwrap();
            assert!(
                ClientBridge::start(&f.client_cfg, &f.host).await.is_err(),
                "host key cannot assume controller role"
            );
            let _viewer = ClientBridge::start(&f.client_cfg, &f.viewer).await.unwrap();
            let mut bad = f.client_cfg.clone();
            bad.token = "ff".repeat(32);
            assert!(
                ClientBridge::start(&bad, &f.viewer).await.is_err(),
                "an unexpected/wrong token must not downgrade authentication"
            );
        }
    }
}

#[tokio::test]
async fn websocket_device_proof_cannot_be_replayed_on_a_new_connection() {
    use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};
    let f = Fixture::secure_transport(true, None, 1).await;
    let request = || {
        let mut request = f
            .host_cfg
            .websocket_url()
            .unwrap()
            .into_client_request()
            .unwrap();
        let h = request.headers_mut();
        h.insert("x-removent-room", "office".parse().unwrap());
        h.insert("x-removent-role", "host".parse().unwrap());
        h.insert(
            "x-removent-key",
            hex::encode(f.host.verifying_key().as_bytes())
                .parse()
                .unwrap(),
        );
        h.insert(
            "sec-websocket-protocol",
            removent_relay::websocket::PROTOCOL.parse().unwrap(),
        );
        request
    };
    let (mut socket, _) = tokio_tungstenite::connect_async(request()).await.unwrap();
    let Message::Binary(challenge) = socket.next().await.unwrap().unwrap() else {
        panic!();
    };
    let proof = removent_relay::auth::sign_challenge(
        &f.host,
        "office",
        Role::Host,
        &challenge.as_ref().try_into().unwrap(),
    );
    drop(socket);
    let (mut socket, _) = tokio_tungstenite::connect_async(request()).await.unwrap();
    let _fresh = socket.next().await.unwrap().unwrap();
    socket
        .send(Message::Binary(proof.to_vec().into()))
        .await
        .unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .unwrap();
    assert!(
        !matches!(reply, Some(Ok(Message::Binary(_)))),
        "replay must never receive a route"
    );
}

#[tokio::test]
async fn relay_cannot_redirect_a_pinned_viewer_to_a_different_rvp_host() {
    for websocket in [false, true] {
        let f = Fixture::transport(websocket, None).await;
        // Routing authentication succeeds; the independent inner host pin must fail.
        let (host_ep, _) = make_server_endpoint(
            "127.0.0.1:0".parse().unwrap(),
            &f.host,
            PinState::new([f.viewer.fingerprint], false),
        )
        .unwrap();
        let host_tunnel = client::connect_host(&f.host_cfg, &f.host).await.unwrap();
        let _host_task = HostTask(tokio::spawn(host_tunnel.run(host_ep.local_addr().unwrap())));
        let bridge = ClientBridge::start(&f.client_cfg, &f.viewer).await.unwrap();
        let (client_ep, _) = make_client_endpoint(
            "127.0.0.1:0".parse().unwrap(),
            &f.viewer,
            PinState::new([f.viewer.fingerprint], false),
        )
        .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            let (viewer, host) = tokio::join!(
                async { client_ep.connect(bridge.address, "removent").unwrap().await },
                async { host_ep.accept().await.unwrap().await }
            );
            assert!(viewer.is_err());
            assert!(host.is_err());
        })
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn network_allowlist_rejects_real_peers_before_auth_on_both_carriers() {
    for websocket in [false, true] {
        let denied =
            Fixture::with_cidrs(websocket, None, 0, vec!["192.0.2.0/24".parse().unwrap()]).await;
        assert!(
            client::connect_host(&denied.host_cfg, &denied.host)
                .await
                .is_err()
        );
        let allowed =
            Fixture::with_cidrs(websocket, None, 0, vec!["127.0.0.0/8".parse().unwrap()]).await;
        assert!(
            client::connect_host(&allowed.host_cfg, &allowed.host)
                .await
                .is_ok()
        );
    }
}
