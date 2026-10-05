use super::*;

/// Loopback with caps.audio=false: negotiation must settle on a single (video-only)
/// media stream — the stream layout the app actually runs, previously untested.
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn audio_disabled_session_uses_single_media_stream() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("removent_host=debug,removent_client=debug")
        .with_test_writer()
        .try_init();
    let (client_id, _cd) = identity_for("AudioOffClient");
    let (host_id, _hd) = identity_for("AudioOffHost");

    let (ep_server, _server_pin) = make_server_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &host_id,
        PinState::new([], true),
    )
    .unwrap();
    let server_addr = ep_server.local_addr().unwrap();

    let client_fp = client_id.fingerprint_hex();
    let host_task = tokio::spawn(async move {
        let incoming = ep_server.accept().await.expect("incoming");
        let conn = RvpConnection::new(incoming.await.expect("conn"));

        // Pre-trusted peer: no pairing, no admission prompt.
        let mut peers = PeersStore::in_memory();
        peers
            .upsert(removent_core::PeerRecord {
                fingerprint: client_fp.clone(),
                name: "AudioOffClient".into(),
                short_fp: client_fp.chars().take(16).collect(),
                granted_caps: Caps::all(),
                trusted: true,
                added_at_unix: 0,
                last_connected_unix: 0,
            })
            .unwrap();
        let cfg = HostConfig {
            authentication: Default::default(),
            auth_paths: None,
            audio_available: true,
            preapproved_only: false,
            device_name: "AudioOffHost".into(),
            admission: AdmissionMode::AlwaysAsk,
            video_bitrate_kbps: 3_000,
            video_fps: 30,
            input_sink: None,
            local_clip: Some(removent_core::MemoryClipboard::new()),
        };
        let interactions = HostInteractions {
            show_pairing_pin: Box::new(|_| panic!("trusted peer must not pair")),
            admission_prompt: Box::new(|_, _| {
                Box::pin(async { true }) as std::pin::Pin<Box<dyn Future<Output = bool> + Send>>
            }),
        };
        let display = removent_proto::DisplayInfo {
            id: 1,
            w_px: W as u32,
            h_px: H as u32,
            scale: 2.0,
            dpi: 192,
            is_main: true,
        };
        let (established, kf_rx, quality_rx, cmd_rx, sink, source) = serve_connection(
            conn.clone(),
            &host_id,
            &mut peers,
            &cfg,
            interactions,
            display,
        )
        .await
        .expect("serve");

        assert!(
            established.clip_state.is_none(),
            "declined clipboard must not start a host poller"
        );
        // The peer declined audio: exactly one media stream, no audio loop (§5.2).
        assert!(
            !established.ack.audio.enabled,
            "audio must be negotiated off"
        );
        let deps = removent_host::ControlPumpDeps {
            conn: conn.clone(),
            kf_tx: None,
            controller: None,
            window_ms: 250,
            input: None,
            local_clip: None,
            quality_tx: None,
            caps: established.peer_caps,
            clip_state: established.clip_state.clone(),
            cancel: established.cancel.clone(),
            peer_fp: Some(established.peer_fp_hex.clone()),
            delivery: None,
        };
        removent_host::spawn_control_pump(source, sink, deps, cmd_rx);

        let (video_tx, video_rx) = removent_core::latest::channel::<(Vec<u8>, i64)>();
        let vstream = conn.open_media_stream().await.unwrap();
        let vhandle = spawn_video_loop(
            vstream,
            video_rx,
            kf_rx,
            quality_rx,
            established.ack.video.codec,
            W,
            H,
            established.ack.video.max_bitrate_kbps,
            established.ack.video.max_fps,
            established.cancel.clone(),
            None,
            0,
            None,
            None,
        );
        for t in 0..10usize {
            if video_tx
                .send((synthetic_bgra(t), (t as i64) * 33_333))
                .is_err()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        let _keep = (conn, video_tx);
        vhandle.await.expect("video loop");
    });

    let (ep_client, _client_pin) = make_client_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &client_id,
        PinState::new([], true),
    )
    .unwrap();
    let mut session = connect_session(
        ep_client.clone(),
        server_addr,
        &client_id,
        ClientConfig {
            pairing_code: None,
            device_name: "AudioOffClient".into(),
            caps: Caps {
                audio: false,
                clipboard: false,
                ..Caps::all()
            },
            local_clip: None,
        },
        None,
        None,
        None,
    )
    .await
    .expect("client connect");

    assert!(
        !session.negotiated.audio.enabled,
        "client ack must have audio disabled"
    );

    // Verify stream layout and delivery, not hardware startup latency. A cold
    // VideoToolbox session on a busy machine can consume most of ten seconds.
    // Keep a bounded wait while allowing initialization under build load.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut latest_pts = None;
    while tokio::time::Instant::now() < deadline && latest_pts != Some(9 * 33_333) {
        match tokio::time::timeout(Duration::from_millis(500), session.decoded_bgra_rx.recv()).await
        {
            Ok(Some(frame)) => {
                assert_eq!(frame.data.len(), W * H * 4);
                latest_pts = Some(frame.pts_us);
            }
            Ok(None) => break,
            Err(_) => {}
        }
    }
    // Coalescing may skip intermediate captures. The final capture must still
    // reach the viewer, including when no more callbacks arrive afterwards.
    assert_eq!(latest_pts, Some(9 * 33_333));

    session.close().await.expect("close");
    host_task.abort();
}
