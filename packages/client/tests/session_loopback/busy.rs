use super::*;

/// Busy end-to-end: while a session is active, a second connection must receive
/// SessionReject{Busy} fast (not hang), and the client must surface Rejected("Busy")
/// rather than a pairing-stream read error. Uses a trusted re-connecting identity so
/// peer_known=true: no PIN may be requested on the busy path either.
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn second_connection_gets_busy_reject() {
    let (client_id, _cd) = identity_for("BusyClient");
    let (host_id, _hd) = identity_for("BusyHost");
    let host_dir = tempfile::tempdir().unwrap();
    let host_paths = DataPaths {
        root: host_dir.path().to_path_buf(),
    };

    let (ep_server, _server_pin) = make_server_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &host_id,
        PinState::new([], true),
    )
    .unwrap();
    let server_addr = ep_server.local_addr().unwrap();

    let (pin_tx, pin_from_host) = oneshot::channel::<String>();
    let (established_tx, established_rx) = oneshot::channel::<()>();

    let host_paths_for_task = host_paths.clone();
    let host_task = tokio::spawn(async move {
        let mut peers = PeersStore::load(&host_paths_for_task).expect("peers load");
        let display = removent_proto::DisplayInfo {
            id: 1,
            w_px: W as u32,
            h_px: H as u32,
            scale: 2.0,
            dpi: 192,
            is_main: true,
        };
        let cfg = HostConfig {
            audio_available: true,
            preapproved_only: false,
            device_name: "BusyHost".into(),
            admission: AdmissionMode::AlwaysAsk,
            video_bitrate_kbps: 3_000,
            video_fps: 30,
            input_sink: None,
            local_clip: None,
        };
        // First connection: pairing + admission (auto-allow), then held open so the
        // second connection finds the host busy.
        let incoming1 = ep_server.accept().await.expect("incoming1");
        let conn1 = RvpConnection::new(incoming1.await.expect("conn1"));
        let interactions = HostInteractions {
            show_pairing_pin: Box::new(move |pin| {
                let _ = pin_tx.send(pin);
            }),
            admission_prompt: Box::new(|_, _| {
                Box::pin(async { true }) as std::pin::Pin<Box<dyn Future<Output = bool> + Send>>
            }),
        };
        let (est1, _kf, _br, _cr, sink1, src1) = serve_connection(
            conn1.clone(),
            &host_id,
            &mut peers,
            &cfg,
            interactions,
            display,
        )
        .await
        .expect("serve1");
        established_tx.send(()).expect("established signal");

        // Second connection arrives while the session slot is taken: answer Busy.
        let incoming2 = ep_server.accept().await.expect("incoming2");
        let conn2 = RvpConnection::new(incoming2.await.expect("conn2"));
        removent_host::reject_busy(conn2, "BusyHost".into(), host_paths_for_task.clone());

        // Keep the first session (and its control stream) alive until the test is done.
        let _keep = (conn1, est1, sink1, src1);
        tokio::time::sleep(Duration::from_secs(30)).await;
    });

    // PIN forwarding for the first (pairing) connection.
    let pin_request: removent_client::PinRequest = Box::new(move |pin_tx| {
        tokio::spawn(async move {
            if let Ok(pin) = pin_from_host.await {
                let _ = pin_tx.send(pin);
            }
        });
    });

    let (ep_client, _client_pin) = make_client_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &client_id,
        PinState::new([], true),
    )
    .unwrap();
    let mk_cfg = || ClientConfig {
        device_name: "BusyClient".into(),
        caps: Caps::all(),
        local_clip: None,
    };

    let session1 = connect_session(
        ep_client.clone(),
        server_addr,
        &client_id,
        mk_cfg(),
        None,
        None,
        Some(pin_request),
    )
    .await
    .expect("first connect");
    tokio::time::timeout(Duration::from_secs(5), established_rx)
        .await
        .expect("host established timeout")
        .expect("host established");

    // Same (now trusted) identity reconnects while the host is busy: it must be
    // rejected fast with Busy — no hang, no PIN request, no masked pairing error.
    let connect2 = tokio::time::timeout(
        Duration::from_secs(10),
        connect_session(
            ep_client.clone(),
            server_addr,
            &client_id,
            mk_cfg(),
            None,
            None,
            Some(Box::new(|_: oneshot::Sender<String>| {
                panic!("busy-rejected trusted peer must not request a PIN");
            })),
        ),
    )
    .await
    .expect("busy connect must fail fast, not hang");
    let err = match connect2 {
        Err(e) => e,
        Ok(_) => panic!("second connection must be rejected"),
    };
    match err {
        removent_client::ConnectError::Rejected(reason) => {
            assert!(
                reason.contains("Busy"),
                "expected Busy reject, got {reason}"
            );
        }
        other => panic!("expected Rejected(Busy), got {other:?}"),
    }

    session1.close().await.expect("close");
    host_task.abort();
}
