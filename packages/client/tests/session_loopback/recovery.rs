use super::*;

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
            audio_available: true,
            preapproved_only: false,
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
