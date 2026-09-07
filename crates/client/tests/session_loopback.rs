//! Local loopback E2E: pairing → admission → negotiation → video/audio decode → resume token.

use removent_client::{ClientConfig, connect_session, quick_resume};
use removent_core::{
    AdmissionMode, DataPaths, DeviceIdentity, PeersStore, QualityPreset,
    adapt::AdaptationController, identity,
};
use removent_host::{
    HostConfig, HostInteractions, serve_connection, spawn_audio_loop, spawn_video_loop,
};
use removent_net::{PinState, RvpConnection, make_client_endpoint, make_server_endpoint};
use removent_proto::{Caps, CodecId};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

const W: usize = 320;
const H: usize = 240;

fn identity_for(name: &str) -> (DeviceIdentity, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let p = DataPaths {
        root: dir.path().to_path_buf(),
    };
    (identity::load_or_create(&p, name).unwrap(), dir)
}

fn synthetic_bgra(t: usize) -> Vec<u8> {
    let mut buf = vec![0u8; W * H * 4];
    for y in 0..H {
        for x in 0..W {
            let i = (y * W + x) * 4;
            buf[i] = ((x + t * 13) % 256) as u8;
            buf[i + 1] = ((y + t * 5) % 256) as u8;
            buf[i + 2] = 77;
            buf[i + 3] = 255;
        }
    }
    buf
}

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
    let pin_request: removent_client::PinRequest = Box::new(move |pin_tx| {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_rejects_unknown_token() {
    let (host_id, _hd) = identity_for("E2EHost2");
    let bogus = [7u8; 16];
    assert!(removent_host::validate_resume_for("unknown-fp", &bogus).is_none());
    let _ = host_id;
}

/// Quick-resume real reconnect: first connect pairs and gets a token → close →
/// quick_resume recovers without pairing, and the host rotates the token (the old token
/// is single-use; the new token reaches the client via NegotiateReply).
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn quick_resume_reconnects_and_rotates_token() {
    let (client_id, _cd) = identity_for("ResumeClient");
    let (host_id, _hd) = identity_for("ResumeHost");

    let (ep_server, _server_pin) = make_server_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &host_id,
        PinState::new([], true),
    )
    .unwrap();
    let server_addr = ep_server.local_addr().unwrap();

    let (pin_tx, pin_from_host) = oneshot::channel::<String>();
    let pin_tx = std::sync::Mutex::new(Some(pin_tx));
    let (report_tx, mut report_rx) = mpsc::channel::<(String, [u8; 16], [u8; 16])>(1);

    let host_task = tokio::spawn(async move {
        let mut peers = PeersStore::in_memory();
        let display = removent_proto::DisplayInfo {
            id: 1,
            w_px: W as u32,
            h_px: H as u32,
            scale: 2.0,
            dpi: 192,
            is_main: true,
        };
        let mk_cfg = || HostConfig {
            device_name: "ResumeHost".into(),
            admission: AdmissionMode::AlwaysAsk,
            video_bitrate_kbps: 3_000,
            video_fps: 30,
            input_sink: None,
            local_clip: None,
        };
        // First connection: pairing + admission (auto-allow).
        let incoming1 = ep_server.accept().await.expect("incoming1");
        let conn1 = RvpConnection::new(incoming1.await.expect("conn1"));
        let interactions1 = {
            let pin_tx = pin_tx.lock().unwrap().take().expect("pin tx");
            HostInteractions {
                show_pairing_pin: Box::new(move |pin| {
                    let _ = pin_tx.send(pin);
                }),
                admission_prompt: Box::new(|_, _| {
                    Box::pin(async { true }) as std::pin::Pin<Box<dyn Future<Output = bool> + Send>>
                }),
            }
        };
        let (est1, _kf1, _br1, _cr1, _sink1, _src1) = serve_connection(
            conn1.clone(),
            &host_id,
            &mut peers,
            &mk_cfg(),
            interactions1,
            display.clone(),
        )
        .await
        .expect("serve1");

        // Resumed connection: skips pairing/admission, directly Accept + rotate token.
        let incoming2 = ep_server.accept().await.expect("incoming2");
        let conn2 = RvpConnection::new(incoming2.await.expect("conn2"));
        let (est2, _kf2, _br2, _cr2, _sink2, _src2) = serve_connection(
            conn2.clone(),
            &host_id,
            &mut peers,
            &mk_cfg(),
            HostInteractions {
                show_pairing_pin: Box::new(|_| panic!("resume must skip pairing")),
                admission_prompt: Box::new(|_, _| {
                    Box::pin(async {
                        panic!("resume must skip admission");
                        #[allow(unreachable_code)]
                        false
                    }) as std::pin::Pin<Box<dyn Future<Output = bool> + Send>>
                }),
            },
            display,
        )
        .await
        .expect("serve2 resume");
        let report = (
            est1.peer_fp_hex.clone(),
            est1.resume_token,
            est2.resume_token,
        );
        report_tx.send(report).await.expect("report");
        let _keep = (conn1, conn2, est1, est2);
        tokio::time::sleep(Duration::from_secs(30)).await;
    });

    // PIN forwarding (only when pairing actually asks for it).
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
        device_name: "ResumeClient".into(),
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
    let token1 = session1.current_resume_token().expect("token1");
    let ack1 = session1.negotiated.clone();
    session1.close().await.expect("close");

    let session2 = quick_resume(
        ep_client.clone(),
        server_addr,
        &client_id,
        mk_cfg(),
        token1,
        ack1,
    )
    .await
    .expect("quick resume");

    let (peer_fp, host_token1, host_token2) =
        tokio::time::timeout(Duration::from_secs(10), report_rx.recv())
            .await
            .expect("host report timeout")
            .expect("host report");

    assert_eq!(host_token1, token1, "host issued token matches client");
    assert_ne!(host_token2, token1, "token must rotate on resume");

    // The consumed token survives one more validation as the tolerated previous
    // generation (a lost rotation reply must not strand the client, §7.4); the
    // rotated token is the current one.
    assert!(removent_host::validate_resume_for(&peer_fp, &host_token2).is_some());
    assert!(
        removent_host::validate_resume_for(&peer_fp, &token1).is_some(),
        "previous-generation token is tolerated once"
    );

    // The client receives the rotated new token via the pump.
    let dl = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        if session2.current_resume_token() == Some(host_token2) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < dl,
            "client never received rotated token"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    session2.close().await.expect("close2");
    host_task.abort();
}

/// Negative path: wrong PIN → the client must attribute the failure to Pairing, and the
/// host side rejects pairing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wrong_pin_fails_with_pairing_error() {
    let (client_id, _cd) = identity_for("WrongPinClient");
    let (host_id, _hd) = identity_for("WrongPinHost");

    let (ep_server, _server_pin) = make_server_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &host_id,
        PinState::new([], true),
    )
    .unwrap();
    let server_addr = ep_server.local_addr().unwrap();

    let (pin_tx, pin_from_host) = oneshot::channel::<String>();

    let host_task = tokio::spawn(async move {
        let incoming = ep_server.accept().await.expect("incoming");
        let conn = RvpConnection::new(incoming.await.expect("conn"));
        let mut peers = PeersStore::in_memory();
        let cfg = HostConfig {
            device_name: "WrongPinHost".into(),
            admission: AdmissionMode::AlwaysAsk,
            video_bitrate_kbps: 3_000,
            video_fps: 30,
            input_sink: None,
            local_clip: None,
        };
        let interactions = HostInteractions {
            show_pairing_pin: Box::new(move |pin| {
                let _ = pin_tx.send(pin);
            }),
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
        let result = serve_connection(
            conn.clone(),
            &host_id,
            &mut peers,
            &cfg,
            interactions,
            display,
        )
        .await;
        let _keep = conn;
        match result {
            Err(e) => e.to_string(),
            Ok(_) => panic!("wrong pin must fail serve"),
        }
    });

    // After getting the PIN shown by the host, the client deliberately enters it wrong.
    let pin_request: removent_client::PinRequest = Box::new(move |pin_tx| {
        tokio::spawn(async move {
            if let Ok(pin) = pin_from_host.await {
                let wrong = if pin == "000000" { "000001" } else { "000000" };
                let _ = pin_tx.send(wrong.to_string());
            }
        });
    });

    let (ep_client, _client_pin) = make_client_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &client_id,
        PinState::new([], true),
    )
    .unwrap();
    let err = match connect_session(
        ep_client.clone(),
        server_addr,
        &client_id,
        ClientConfig {
            device_name: "WrongPinClient".into(),
            caps: Caps::all(),
            local_clip: None,
        },
        None,
        None,
        Some(pin_request),
    )
    .await
    {
        Err(e) => e,
        Ok(_) => panic!("wrong pin must fail connect"),
    };
    assert!(
        matches!(err, removent_client::ConnectError::Pairing(_)),
        "expected Pairing error, got {err:?}"
    );

    let host_err = tokio::time::timeout(Duration::from_secs(10), host_task)
        .await
        .expect("host task timeout")
        .expect("host task panic");
    assert!(host_err.contains("pin mismatch"), "{host_err}");
}

/// Regression: a resume token the host rejects (bogus/expired) must NOT desync the
/// two ends. The host answers `resume_accepted = Some(false)` in the handshake and
/// waits out the full negotiation; the client must fall back to that path and answer
/// the NegotiateOffer (previously it jumped straight into a session on SessionAccept,
/// leaving the host to time out after 10s: "connected → black screen → drop").
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn resume_rejected_falls_back_to_full_negotiation() {
    let (client_id, _cd) = identity_for("RejectClient");
    let (host_id, _hd) = identity_for("RejectHost");

    let (ep_server, _server_pin) = make_server_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &host_id,
        PinState::new([], true),
    )
    .unwrap();
    let server_addr = ep_server.local_addr().unwrap();

    let (pin_tx, pin_from_host) = oneshot::channel::<String>();
    let pin_tx = std::sync::Mutex::new(Some(pin_tx));
    let (report_tx, mut report_rx) = mpsc::channel::<Result<[u8; 16], String>>(1);

    let host_task = tokio::spawn(async move {
        let mut peers = PeersStore::in_memory();
        let display = removent_proto::DisplayInfo {
            id: 1,
            w_px: W as u32,
            h_px: H as u32,
            scale: 2.0,
            dpi: 192,
            is_main: true,
        };
        let mk_cfg = || HostConfig {
            device_name: "RejectHost".into(),
            admission: AdmissionMode::AlwaysAsk,
            video_bitrate_kbps: 3_000,
            video_fps: 30,
            input_sink: None,
            local_clip: None,
        };
        // First connection: pairing + admission (auto-allow) → the peer becomes trusted.
        let incoming1 = ep_server.accept().await.expect("incoming1");
        let conn1 = RvpConnection::new(incoming1.await.expect("conn1"));
        let interactions1 = {
            let pin_tx = pin_tx.lock().unwrap().take().expect("pin tx");
            HostInteractions {
                show_pairing_pin: Box::new(move |pin| {
                    let _ = pin_tx.send(pin);
                }),
                admission_prompt: Box::new(|_, _| {
                    Box::pin(async { true }) as std::pin::Pin<Box<dyn Future<Output = bool> + Send>>
                }),
            }
        };
        let (est1, _kf1, _br1, _cr1, _sink1, _src1) = serve_connection(
            conn1.clone(),
            &host_id,
            &mut peers,
            &mk_cfg(),
            interactions1,
            display.clone(),
        )
        .await
        .expect("serve1");

        // Second connection: the client presents a bogus token. The host must reject
        // the token but still complete full negotiation with the (trusted) peer —
        // no pairing, no admission prompt, no 10s NegotiateReply timeout.
        let incoming2 = ep_server.accept().await.expect("incoming2");
        let conn2 = RvpConnection::new(incoming2.await.expect("conn2"));
        let served2 = serve_connection(
            conn2.clone(),
            &host_id,
            &mut peers,
            &mk_cfg(),
            HostInteractions {
                show_pairing_pin: Box::new(|_| panic!("trusted peer must not pair again")),
                admission_prompt: Box::new(|_, _| {
                    Box::pin(async {
                        panic!("trusted peer must skip admission");
                        #[allow(unreachable_code)]
                        false
                    }) as std::pin::Pin<Box<dyn Future<Output = bool> + Send>>
                }),
            },
            display,
        )
        .await;
        let report = served2
            .map(|(est2, ..)| est2.resume_token)
            .map_err(|e| e.to_string());
        report_tx.send(report).await.expect("report");
        let _keep = (conn1, conn2, est1);
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
        device_name: "RejectClient".into(),
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
    let token1 = session1.current_resume_token().expect("token1");
    let ack1 = session1.negotiated.clone();
    session1.close().await.expect("close");

    // Reconnect with a bogus token: the peer is trusted, so no PIN may be requested,
    // and the session must come up through full negotiation (fast, no 10s desync).
    let bogus = [9u8; 16];
    let session2 = tokio::time::timeout(
        Duration::from_secs(10),
        connect_session(
            ep_client.clone(),
            server_addr,
            &client_id,
            mk_cfg(),
            Some(bogus),
            Some(ack1),
            Some(Box::new(|_: oneshot::Sender<String>| {
                panic!("known peer must not request a PIN");
            })),
        ),
    )
    .await
    .expect("rejected-resume connect timed out (protocol desync)")
    .expect("rejected resume must fall back to full negotiation");

    let host_token2 = tokio::time::timeout(Duration::from_secs(10), report_rx.recv())
        .await
        .expect("host report timeout")
        .expect("host report")
        .expect("host must complete negotiation instead of timing out");

    // Full negotiation issued a fresh token (the stale one is not reused).
    assert_ne!(host_token2, token1, "host must rotate a fresh token");
    assert_eq!(
        session2.current_resume_token(),
        Some(host_token2),
        "client must adopt the freshly negotiated token"
    );
    session2.close().await.expect("close2");
    host_task.abort();
}

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

/// Loopback with caps.audio=false: negotiation must settle on a single (video-only)
/// media stream — the stream layout the app actually runs, previously untested.
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn audio_disabled_session_uses_single_media_stream() {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_connect_aborts_pairing_while_pin_dialog_is_still_open() {
    let (client_id, _cd) = identity_for("CancelClient");
    let (host_id, _hd) = identity_for("CancelHost");
    let (server, _) = make_server_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &host_id,
        PinState::new([], true),
    )
    .unwrap();
    let addr = server.local_addr().unwrap();
    let (shown_tx, shown_rx) = oneshot::channel();
    let host = tokio::spawn(async move {
        let conn = RvpConnection::new(server.accept().await.unwrap().await.unwrap());
        let mut peers = PeersStore::in_memory();
        let cfg = HostConfig {
            device_name: "CancelHost".into(),
            admission: AdmissionMode::AlwaysAsk,
            video_bitrate_kbps: 3000,
            video_fps: 30,
            input_sink: None,
            local_clip: None,
        };
        let interactions = HostInteractions {
            show_pairing_pin: Box::new(move |_| {
                let _ = shown_tx.send(());
            }),
            admission_prompt: Box::new(|_, _| Box::pin(async { false })),
        };
        let display = removent_proto::DisplayInfo {
            id: 1,
            w_px: W as u32,
            h_px: H as u32,
            scale: 1.,
            dpi: 96,
            is_main: true,
        };
        serve_connection(conn, &host_id, &mut peers, &cfg, interactions, display)
            .await
            .is_err()
    });
    let (endpoint, _) = make_client_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &client_id,
        PinState::new([], true),
    )
    .unwrap();
    let (prompt_tx, prompt_rx) = oneshot::channel();
    let client = tokio::spawn(async move {
        connect_session(
            endpoint,
            addr,
            &client_id,
            ClientConfig {
                device_name: "CancelClient".into(),
                caps: Caps::all(),
                local_clip: None,
            },
            None,
            None,
            Some(Box::new(move |pin_tx| {
                let _ = prompt_tx.send(pin_tx);
            })),
        )
        .await
    });
    // Keep the PIN sender alive: cancellation must stop pairing independently
    // of whether the UI has already destroyed the dialog.
    let _pin_dialog = tokio::time::timeout(Duration::from_secs(5), prompt_rx)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), shown_rx)
        .await
        .unwrap()
        .unwrap();
    client.abort();
    assert!(matches!(client.await, Err(e) if e.is_cancelled()));
    assert!(
        tokio::time::timeout(Duration::from_secs(2), host)
            .await
            .expect("cancelled pairing must release the host promptly")
            .unwrap()
    );
}
