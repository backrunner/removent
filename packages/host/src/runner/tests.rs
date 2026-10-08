use super::*;
use removent_proto::{HandshakeClient, Hello};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_reaps_connection_waiting_for_pairing() {
    let dir = tempfile::tempdir().unwrap();
    let paths = DataPaths {
        root: dir.path().to_owned(),
    };
    let id = identity::load_or_create(&paths, "shutdown-test").unwrap();
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = socket.local_addr().unwrap();
    drop(socket);
    let stop = CancellationToken::new();
    // Callbacks are retained by ConnectionCtx: a leaked negotiation task
    // would keep this sentinel alive after serve_forever returns.
    let sentinel = Arc::new(());
    let weak = Arc::downgrade(&sentinel);
    let cfg = HostRunnerConfig {
        paths,
        settings: Settings {
            host_port: addr.port(),
            vnc_enabled: false,
            ..Settings::default()
        },
        input_sink: None,
        local_clip: None,
    };
    let mut task = tokio::spawn(serve_forever(
        cfg,
        HostCallbacks {
            show_pairing_pin: Box::new(|_| {}),
            admission_prompt: Box::new(|_, _| Box::pin(async { true })),
            on_event: Box::new(move |_| {
                let _keep = &sentinel;
            }),
        },
        Arc::new(AtomicBool::new(true)),
        stop.clone(),
    ));
    let (client, _) = removent_net::make_client_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &id,
        PinState::new([], true),
    )
    .unwrap();
    let conn = tokio::time::timeout(
        Duration::from_secs(5),
        client.connect(addr, "removent").unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    let conn = RvpConnection::new(conn);
    let _control = conn
        .connect_handshake(HandshakeClient {
            magic: removent_proto::MAGIC,
            proto_version: PROTO_VERSION,
            feature_bits: 0,
            hello: Hello {
                app_version: "test".into(),
                device_name: "test".into(),
                os_version: "test".into(),
                caps: Caps::all(),
                resume_token: None,
            },
        })
        .await
        .unwrap();
    // No pairing stream arrives. Previously this task survived host stop
    // for the full pairing deadline and retained the advertiser/resources.
    stop.cancel();
    tokio::time::timeout(Duration::from_secs(2), &mut task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        weak.upgrade().is_none(),
        "negotiation task retained runner callbacks"
    );
    tokio::time::timeout(Duration::from_secs(1), conn.inner().closed())
        .await
        .unwrap();
}
