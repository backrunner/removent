use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn full_session_pair_negotiate_media() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,removent=debug".into()),
        )
        .with_test_writer()
        .try_init();
    let (client_id, _cd) = identity_for("E2EClient");
    let (host_id, _hd) = identity_for("E2EHost");

    let (ep_server, _server_pin) = make_server_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &host_id,
        PinState::new([], true),
    )
    .unwrap();
    let server_addr = ep_server.local_addr().unwrap();

    let admission_calls = Arc::new(AtomicUsize::new(0));
    let admission_h = admission_calls.clone();
    let (pin_tx, pin_from_host) = oneshot::channel::<String>();
    static HOST_CLIP: std::sync::Mutex<Option<Arc<dyn removent_core::TextClipboard>>> =
        std::sync::Mutex::new(None);
    *HOST_CLIP.lock().unwrap() = None;
    let recorder = Arc::new(removent_host::RecorderInputSink::default());
    let recorder_for_assert = recorder.clone();

    // ---- host task: accept → serve → media loops → feed frames ----
    let host_task = tokio::spawn(async move {
        let incoming = ep_server.accept().await.expect("incoming");
        let qconn = incoming.await.expect("host handshake");
        let conn = RvpConnection::new(qconn);

        let mut peers = PeersStore::in_memory();
        let host_clip: Arc<dyn removent_core::TextClipboard> =
            removent_core::MemoryClipboard::new();
        let host_clip_for_pump = host_clip.clone();
        let recorder_for_pump = recorder.clone();
        let host_clip_for_cfg = host_clip.clone();
        *HOST_CLIP.lock().unwrap() = Some(host_clip);
        let cfg = HostConfig {
            authentication: Default::default(),
            auth_paths: None,
            audio_available: true,
            preapproved_only: false,
            device_name: "HostLoop".into(),
            admission: AdmissionMode::AlwaysAsk,
            video_bitrate_kbps: 3_000,
            video_fps: 30,
            input_sink: Some(recorder),
            local_clip: Some(host_clip_for_cfg),
        };
        let interactions = HostInteractions {
            show_pairing_pin: Box::new(move |pin| {
                let _ = pin_tx.send(pin);
            }),
            admission_prompt: Box::new(move |_name, _fp| {
                admission_h.fetch_add(1, Ordering::SeqCst);
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

        // Control pump: inject input / apply clipboard / adapt.
        let controller = Arc::new(std::sync::Mutex::new(AdaptationController::new(
            3_000,
            30,
            QualityPreset::Auto,
        )));
        {
            let deps = removent_host::ControlPumpDeps {
                conn: conn.clone(),
                kf_tx: None,
                controller: Some(controller.clone()),
                window_ms: 250,
                input: Some(recorder_for_pump),
                local_clip: Some(host_clip_for_pump),
                quality_tx: None,
                caps: established.peer_caps,
                clip_state: established.clip_state.clone(),
                cancel: established.cancel.clone(),
                peer_fp: Some(established.peer_fp_hex.clone()),
                delivery: None,
            };
            removent_host::spawn_control_pump(source, sink, deps, cmd_rx);
        }

        // Media loops (order: video first, then audio; the client accepts in this order).
        let (video_tx, video_rx) = removent_core::latest::channel::<(Vec<u8>, i64)>();
        let (audio_tx, audio_rx) = mpsc::channel::<removent_media_capture::AudioFrame>(16);
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
        let astream = conn.open_media_stream().await.unwrap();
        let ahandle = spawn_audio_loop(
            astream,
            audio_rx,
            established.ack.audio.bitrate_kbps,
            established.cancel.clone(),
        );

        // Feed the streams.
        for t in 0..10usize {
            if video_tx
                .send((synthetic_bgra(t), (t as i64) * 33_333))
                .is_err()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
        for t in 0..20usize {
            if audio_tx
                .send(removent_media_capture::AudioFrame {
                    samples: vec![100i16; 960],
                    pts_micros: (t as i64) * 10_000,
                })
                .await
                .is_err()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(8)).await;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;

        (
            established.peer_fp_hex,
            established.resume_token,
            vhandle,
            ahandle,
            video_tx,
            audio_tx,
        )
    });

    // ---- PIN forwarding: host displays → client enters (only when pairing asks) ----
    let pin_request: removent_client::PinRequest = Box::new(move |_, pin_tx| {
        tokio::spawn(async move {
            if let Ok(pin) = pin_from_host.await {
                let _ = pin_tx.send(pin);
            }
        });
    });

    // ---- client connect ----
    let (ep_client, _client_pin) = make_client_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &client_id,
        PinState::new([], true),
    )
    .unwrap();
    let client_clip: Arc<dyn removent_core::TextClipboard> = removent_core::MemoryClipboard::new();
    let mut session = connect_session(
        ep_client.clone(),
        server_addr,
        &client_id,
        ClientConfig {
            pairing_code: None,
            device_name: "ClientLoop".into(),
            caps: Caps::all(),
            local_clip: Some(client_clip.clone()),
        },
        None,
        None,
        Some(pin_request),
    )
    .await
    .expect("client connect");

    // Successful pairing grants full-capability trust → no prompt under AlwaysAsk
    // (FR-06 semantics).
    assert_eq!(
        admission_calls.load(Ordering::SeqCst),
        0,
        "trusted peer skips prompt"
    );
    assert!(session.current_resume_token().is_some(), "token issued");

    // ---- input injection: the recorder should capture the client's mouse/keyboard events ----
    session
        .send_input_mouse(1, 100.0, 120.0, 1, removent_proto::MouseKind::LeftDown)
        .await
        .expect("mouse send");
    session
        .send_input_key(
            0x00,
            removent_proto::KeyModifiers::empty(),
            removent_proto::KeyKind::Down,
            None,
        )
        .await
        .expect("key send");

    // ---- clipboard: client → host (explicit send) ----
    session
        .send_clipboard_text(7, "clip-from-client")
        .await
        .expect("clip send");
    tokio::time::sleep(Duration::from_millis(400)).await;
    let expected_codec = if std::env::var("REMOVENT_VIDEO_CODEC")
        .ok()
        .is_some_and(|v| v.eq_ignore_ascii_case("av1") || v.eq_ignore_ascii_case("software-av1"))
    {
        CodecId::Av1
    } else {
        CodecId::Hevc
    };
    assert_eq!(
        session.negotiated.video.codec, expected_codec,
        "host selects the requested codec"
    );

    // ---- receive decoded output ----
    // Video is latest-value: the clipboard wait above deliberately leaves the
    // consumer idle longer than the host's video burst, so only its latest
    // decoded frame is guaranteed to remain. Audio is still a FIFO.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let (mut frames, mut pcm) = (0usize, 0usize);
    while tokio::time::Instant::now() < deadline && (frames < 1 || pcm < 5) {
        tokio::select! {
            maybe = session.decoded_bgra_rx.recv() => match maybe {
                Some(frame) => {
                    assert_eq!(frame.data.len(), W * H * 4);
                    assert_eq!(frame.width as usize, W);
                    assert_eq!(frame.height as usize, H);
                    frames += 1;
                }
                None => break,
            },
            maybe = session.decoded_pcm_rx.recv() => {
                if maybe.is_some() { pcm += 1; }
            }
            _ = tokio::time::sleep(Duration::from_millis(30)) => {}
        }
    }
    assert!(frames >= 1, "decoded frames {frames}");
    assert!(pcm >= 5, "decoded pcm {pcm}");

    // ---- verify input injection ----
    let recorded = recorder_for_assert.events.lock().unwrap().clone();
    assert!(
        recorded.iter().any(|e| matches!(e,
            removent_host::RecordedInput::Mouse { x, y, kind: removent_proto::MouseKind::LeftDown, .. }
            if *x == 100.0 && *y == 120.0)),
        "mouse event should reach host injector, got {recorded:?}"
    );
    assert!(
        recorded.iter().any(|e| matches!(
            e,
            removent_host::RecordedInput::Key {
                vk: 0x00,
                kind: removent_proto::KeyKind::Down,
                ..
            }
        )),
        "key event should reach host injector"
    );

    // ---- adaptation loop: bad stats → host sends QualityControl ----
    for _ in 0..14 {
        session
            .send_stats(30.0, 8.0, 1_000, 40.0)
            .await
            .expect("stats send");
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let dl2 = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let q = *session.last_quality.lock().unwrap();
        if q.is_some() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < dl2,
            "expected QualityControl after degraded stats"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // ---- verify clipboard: client → host ----
    let host_clip = HOST_CLIP
        .lock()
        .unwrap()
        .clone()
        .expect("host clip registered");
    let dl = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        if host_clip
            .read()
            .map(|t| t == "clip-from-client")
            .unwrap_or(false)
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < dl,
            "host clipboard should receive client text"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // ---- resume registry check: the host task returns the real peer fingerprint and token ----
    drop(session);
    let (peer_fp, token, _vh, _ah, _vtx, _atx) =
        tokio::time::timeout(Duration::from_secs(10), host_task)
            .await
            .expect("host task timeout")
            .expect("host task panic");
    let resumed_ack = removent_host::validate_resume_for(&peer_fp, &token);
    assert!(resumed_ack.is_some(), "token must validate within window");
}
