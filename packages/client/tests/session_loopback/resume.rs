use super::*;

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
            audio_available: true,
            preapproved_only: false,
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
