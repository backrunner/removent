use super::*;
use removent_proto::{HandshakeClient, Hello};

#[test]
fn native_and_vnc_slots_are_independent_and_exclusive_per_protocol() {
    let slots = ProtocolSlots::default();
    let native = slots.native.clone().try_acquire_owned().unwrap();
    let vnc = slots.vnc.clone().try_acquire_owned().unwrap();
    assert!(slots.native.clone().try_acquire_owned().is_err());
    assert!(slots.vnc.clone().try_acquire_owned().is_err());
    drop(native);
    assert!(slots.native.clone().try_acquire_owned().is_ok());
    assert!(slots.vnc.clone().try_acquire_owned().is_err());
    drop(vnc);
    assert!(slots.vnc.clone().try_acquire_owned().is_ok());
}

#[test]
fn session_input_isolates_stateful_sinks_and_preserves_recorders() {
    let real: Option<Arc<dyn InputSink>> = Some(Arc::new(crate::RealInputSink::new()));
    let native = session_input(&real).unwrap();
    let vnc = session_input(&real).unwrap();
    assert!(!Arc::ptr_eq(&native, &vnc));
    assert!(!Arc::ptr_eq(&native, real.as_ref().unwrap()));
    let recorder: Option<Arc<dyn InputSink>> = Some(Arc::new(crate::RecorderInputSink::default()));
    assert!(Arc::ptr_eq(
        &session_input(&recorder).unwrap(),
        recorder.as_ref().unwrap()
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_reaps_connection_waiting_for_pairing() {
    use tokio::io::AsyncReadExt;
    let dir = tempfile::tempdir().unwrap();
    let paths = DataPaths {
        root: dir.path().to_owned(),
    };
    let id = identity::load_or_create(&paths, "shutdown-test").unwrap();
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = socket.local_addr().unwrap();
    drop(socket);
    let vnc_socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let vnc_addr = vnc_socket.local_addr().unwrap();
    drop(vnc_socket);
    let stop = CancellationToken::new();
    // Callbacks are retained by ConnectionCtx: a leaked negotiation task
    // would keep this sentinel alive after serve_forever returns.
    let sentinel = Arc::new(());
    let weak = Arc::downgrade(&sentinel);
    let cfg = HostRunnerConfig {
        paths,
        settings: Settings {
            host_port: addr.port(),
            vnc_enabled: true,
            vnc_port: vnc_addr.port(),
            authentication: removent_core::AuthenticationSettings {
                mode: removent_proto::AuthenticationMode::PairingCode,
                ..Default::default()
            },
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
    // A native negotiation holds its protocol slot. VNC must still reach its
    // real RFB handshake, while a second VNC connection is rejected promptly.
    let mut vnc = tokio::net::TcpStream::connect(vnc_addr).await.unwrap();
    let mut greeting = [0; 12];
    tokio::time::timeout(Duration::from_secs(2), vnc.read_exact(&mut greeting))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&greeting, b"RFB 003.008\n");
    let mut duplicate_vnc = tokio::net::TcpStream::connect(vnc_addr).await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(2), duplicate_vnc.read_u8())
        .await
        .unwrap();
    assert!(
        result.is_err(),
        "second VNC session must not get a greeting"
    );
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
    assert!(
        tokio::time::timeout(Duration::from_secs(1), vnc.read_u8())
            .await
            .unwrap()
            .is_err()
    );
}
