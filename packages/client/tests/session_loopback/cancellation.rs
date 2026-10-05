use super::*;

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
            authentication: Default::default(),
            auth_paths: None,
            audio_available: true,
            preapproved_only: false,
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
                pairing_code: None,
                device_name: "CancelClient".into(),
                caps: Caps::all(),
                local_clip: None,
            },
            None,
            None,
            Some(Box::new(move |_, pin_tx| {
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
