use super::*;
use removent_core::{AuthenticationMode as Mode, AuthenticationSettings, PeerRecord};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deny_all_rejects_before_any_credential_prompt_or_otp_consumption() {
    for mode in [Mode::PairingCode, Mode::Password, Mode::Otp, Mode::None] {
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths {
            root: dir.path().into(),
        };
        let auth = AuthenticationSettings {
            mode,
            password: "correct-password".into(),
            otp_secret: "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ".into(),
            ..Default::default()
        };
        assert_eq!(
            attempt(auth, None, false, AdmissionMode::DenyAll, paths.clone()).await,
            (false, 0)
        );
        assert!(!paths.root.join("otp-used.json").exists());
    }
}

async fn attempt(
    auth: AuthenticationSettings,
    input: Option<String>,
    known: bool,
    admission: AdmissionMode,
    paths: DataPaths,
) -> (bool, usize) {
    let (client_id, _cd) = identity_for("AuthClient");
    let (host_id, _hd) = identity_for("AuthHost");
    let client_fp = client_id.fingerprint_hex();
    let (server, _) = make_server_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &host_id,
        PinState::new([], true),
    )
    .unwrap();
    let address = server.local_addr().unwrap();
    let mode = auth.mode;
    let (done_tx, done_rx) = oneshot::channel::<()>();
    let host = tokio::spawn(async move {
        let conn = RvpConnection::new(server.accept().await.unwrap().await.unwrap());
        let mut peers = PeersStore::in_memory();
        if known {
            peers
                .upsert(PeerRecord {
                    fingerprint: client_fp.clone(),
                    name: "known".into(),
                    short_fp: client_fp[..16].into(),
                    trusted: true,
                    granted_caps: Caps::all(),
                    added_at_unix: 0,
                    last_connected_unix: 0,
                })
                .unwrap();
        }
        let cfg = HostConfig {
            authentication: auth,
            auth_paths: Some(paths),
            audio_available: false,
            preapproved_only: false,
            device_name: "AuthHost".into(),
            admission,
            video_bitrate_kbps: 3000,
            video_fps: 30,
            input_sink: None,
            local_clip: None,
        };
        let interactions = HostInteractions {
            show_pairing_pin: Box::new(|_| {
                panic!("unattended modes must not display a pairing PIN")
            }),
            admission_prompt: Box::new(|_, _| {
                panic!("unattended modes must not ask for local approval")
            }),
        };
        let result = serve_connection(
            conn.clone(),
            &host_id,
            &mut peers,
            &cfg,
            interactions,
            removent_proto::DisplayInfo {
                id: 1,
                w_px: W as u32,
                h_px: H as u32,
                scale: 1.,
                dpi: 96,
                is_main: true,
            },
        )
        .await;
        let accepted = result.is_ok();
        if let Ok((session, ..)) = &result {
            assert_eq!(session.peer_name, "AuthClient");
        }
        assert_eq!(
            peers.all().len(),
            usize::from(known),
            "unattended authentication must not create pairing grants"
        );
        let _ = done_rx.await;
        accepted
    });
    let prompts = Arc::new(AtomicUsize::new(0));
    let count = prompts.clone();
    let (endpoint, _) = make_client_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &client_id,
        PinState::new([], true),
    )
    .unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        connect_session(
            endpoint,
            address,
            &client_id,
            ClientConfig {
                pairing_code: None,
                device_name: "AuthClient".into(),
                caps: Caps::all(),
                local_clip: None,
            },
            None,
            None,
            Some(Box::new(move |actual_mode, tx| {
                count.fetch_add(1, Ordering::SeqCst);
                assert_eq!(actual_mode, mode);
                if let Some(input) = input {
                    let _ = tx.send(input);
                }
            })),
        ),
    )
    .await
    .expect("authentication must complete promptly");
    let accepted = result.is_ok();
    let _ = done_tx.send(());
    assert_eq!(host.await.unwrap(), accepted);
    (accepted, prompts.load(Ordering::SeqCst))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn password_authenticates_known_and_unknown_peers_and_rejects_wrong_password() {
    let dir = tempfile::tempdir().unwrap();
    let auth = AuthenticationSettings {
        mode: Mode::Password,
        password: "无人值守 correct horse".into(),
        otp_secret: String::new(),
        ..Default::default()
    };
    for known in [false, true] {
        assert_eq!(
            attempt(
                auth.clone(),
                Some(auth.password.clone()),
                known,
                AdmissionMode::AlwaysAsk,
                DataPaths {
                    root: dir.path().into()
                }
            )
            .await,
            (true, 1)
        );
        assert_eq!(
            attempt(
                auth.clone(),
                Some("wrong password".into()),
                known,
                AdmissionMode::TrustedAuto,
                DataPaths {
                    root: dir.path().into()
                }
            )
            .await,
            (false, 1)
        );
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_authentication_allows_unknown_peers_without_any_prompt_and_respects_deny_all() {
    let dir = tempfile::tempdir().unwrap();
    let auth = AuthenticationSettings {
        mode: Mode::None,
        ..Default::default()
    };
    for admission in [AdmissionMode::AlwaysAsk, AdmissionMode::TrustedAuto] {
        assert_eq!(
            attempt(
                auth.clone(),
                None,
                false,
                admission,
                DataPaths {
                    root: dir.path().into()
                }
            )
            .await,
            (true, 0)
        );
    }
    assert_eq!(
        attempt(
            auth,
            None,
            false,
            AdmissionMode::DenyAll,
            DataPaths {
                root: dir.path().into()
            }
        )
        .await,
        (false, 0)
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn otp_authenticates_and_rejects_reuse_wrong_and_expired_codes() {
    let dir = tempfile::tempdir().unwrap();
    let auth = AuthenticationSettings {
        mode: Mode::Otp,
        otp_secret: AuthenticationSettings::generate_otp_secret(),
        ..Default::default()
    };
    let totp = auth.totp().unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let token = totp.generate(now);
    let candidates: Vec<_> = [now.saturating_sub(30), now, now + 30]
        .into_iter()
        .map(|time| totp.generate(time))
        .collect();
    let wrong = (0..1_000_000)
        .map(|n| format!("{n:06}"))
        .find(|code| !candidates.contains(code))
        .unwrap();
    assert_eq!(
        attempt(
            auth.clone(),
            Some(wrong),
            false,
            AdmissionMode::AlwaysAsk,
            DataPaths {
                root: dir.path().into()
            }
        )
        .await,
        (false, 1)
    );
    assert_eq!(
        attempt(
            auth.clone(),
            Some(token.clone()),
            true,
            AdmissionMode::AlwaysAsk,
            DataPaths {
                root: dir.path().into()
            }
        )
        .await,
        (true, 1)
    );
    assert_eq!(
        attempt(
            auth.clone(),
            Some(token),
            false,
            AdmissionMode::AlwaysAsk,
            DataPaths {
                root: dir.path().into()
            }
        )
        .await,
        (false, 1)
    );
    assert_eq!(
        attempt(
            auth.clone(),
            Some(totp.generate(now.saturating_sub(120))),
            false,
            AdmissionMode::AlwaysAsk,
            DataPaths {
                root: dir.path().into()
            }
        )
        .await,
        (false, 1)
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_password_input_releases_connection_promptly() {
    let dir = tempfile::tempdir().unwrap();
    let auth = AuthenticationSettings {
        mode: Mode::Password,
        password: "secret".into(),
        ..Default::default()
    };
    assert_eq!(
        attempt(
            auth,
            None,
            false,
            AdmissionMode::AlwaysAsk,
            DataPaths {
                root: dir.path().into()
            }
        )
        .await,
        (false, 1)
    );
}
