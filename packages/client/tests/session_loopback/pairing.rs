use super::*;

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
            audio_available: true,
            preapproved_only: false,
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
