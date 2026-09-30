use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_window_admission_uses_available_caps_and_never_prompts() {
    for (input_granted, video_requested) in [(true, true), (false, true), (true, false)] {
        let allowed = input_granted && video_requested;
        let (client_id, _cd) = identity_for("LoginController");
        let (host_id, _hd) = identity_for("LoginHost");
        let (server, _) = make_server_endpoint(
            "127.0.0.1:0".parse().unwrap(),
            &host_id,
            PinState::new([], true),
        )
        .unwrap();
        let addr = server.local_addr().unwrap();
        let client_fp = client_id.fingerprint_hex();
        let (checked_tx, checked_rx) = oneshot::channel();
        let host = tokio::spawn(async move {
            let conn = RvpConnection::new(server.accept().await.unwrap().await.unwrap());
            let mut peers = PeersStore::in_memory();
            peers
                .upsert(removent_core::PeerRecord {
                    fingerprint: client_fp.clone(),
                    name: "LoginController".into(),
                    short_fp: String::new(),
                    granted_caps: Caps {
                        video: video_requested,
                        input: input_granted,
                        ..Caps::none()
                    },
                    trusted: true,
                    added_at_unix: 0,
                    last_connected_unix: 0,
                })
                .unwrap();
            let cfg = HostConfig {
                audio_available: false,
                preapproved_only: true,
                device_name: "LoginHost".into(),
                admission: AdmissionMode::AlwaysAsk,
                video_bitrate_kbps: 3000,
                video_fps: 30,
                input_sink: Some(Arc::new(removent_host::RecorderInputSink::default())),
                local_clip: None,
            };
            let interactions = || HostInteractions {
                show_pairing_pin: Box::new(|_| panic!("LoginWindow must not pair")),
                admission_prompt: Box::new(|_, _| panic!("LoginWindow must not prompt")),
            };
            let display = removent_proto::DisplayInfo {
                id: 1,
                w_px: W as u32,
                h_px: H as u32,
                scale: 1.,
                dpi: 96,
                is_main: true,
            };
            let result = serve_connection(
                conn.clone(),
                &host_id,
                &mut peers,
                &cfg,
                interactions(),
                display.clone(),
            )
            .await;
            if allowed {
                let (session, ..) = result.as_ref().expect("approved video/input must connect");
                assert!(session.peer_caps.video && session.peer_caps.input);
                assert!(
                    !session.peer_caps.audio
                        && !session.peer_caps.clipboard
                        && !session.peer_caps.file
                );
                assert!(!session.ack.audio.enabled);
                assert!(session.clip_state.is_none());
            } else {
                let expected = if video_requested {
                    "prior approval"
                } else {
                    "video capability"
                };
                assert!(
                    matches!(&result, Err(removent_host::HostError::Rejected(reason)) if reason.contains(expected))
                );
            }
            checked_tx.send(()).unwrap();
            conn.inner().closed().await;
            if allowed {
                // A previously valid resume token must not retain a revoked
                // input grant in a service that has no local approval UI.
                let mut peer = peers.by_fingerprint(&client_fp).unwrap().clone();
                peer.granted_caps.input = false;
                peers.upsert(peer).unwrap();
                let conn = RvpConnection::new(server.accept().await.unwrap().await.unwrap());
                let resumed = serve_connection(
                    conn.clone(),
                    &host_id,
                    &mut peers,
                    &cfg,
                    interactions(),
                    display,
                )
                .await;
                assert!(
                    matches!(resumed, Err(removent_host::HostError::Rejected(reason)) if reason.contains("prior approval"))
                );
                conn.inner().closed().await;
            }
        });
        let (endpoint, _) = make_client_endpoint(
            "127.0.0.1:0".parse().unwrap(),
            &client_id,
            PinState::new([], true),
        )
        .unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            connect_session(
                endpoint.clone(),
                addr,
                &client_id,
                ClientConfig {
                    device_name: "LoginController".into(),
                    caps: Caps {
                        video: video_requested,
                        ..Caps::all()
                    },
                    local_clip: None,
                },
                None,
                None,
                None,
            ),
        )
        .await
        .expect("admission must not wait for a local user");
        assert_eq!(result.is_ok(), allowed);
        if let Ok(session) = result {
            assert!(!session.negotiated.audio.enabled);
            let token = session.current_resume_token().unwrap();
            let ack = session.negotiated.clone();
            drop(session);
            let resumed = tokio::time::timeout(
                Duration::from_secs(10),
                quick_resume(
                    endpoint.clone(),
                    addr,
                    &client_id,
                    ClientConfig {
                        device_name: "LoginController".into(),
                        caps: Caps::all(),
                        local_clip: None,
                    },
                    token,
                    ack,
                ),
            )
            .await
            .expect("revoked grants must fail without a prompt");
            assert!(
                resumed.is_err(),
                "a resume token must not restore revoked input"
            );
        }
        tokio::time::timeout(Duration::from_secs(2), checked_rx)
            .await
            .unwrap()
            .unwrap();
        endpoint.close(0u32.into(), b"test finished");
        tokio::time::timeout(Duration::from_secs(2), host)
            .await
            .unwrap()
            .unwrap();
    }
}
