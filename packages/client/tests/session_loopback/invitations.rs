use super::*;
use removent_core::{
    AuthenticationSettings, PeerRecord,
    authentication::PairingPolicy,
    pairing_invitation::{Invitation, PairingCode},
};

async fn pair(
    paths: DataPaths,
    policy: PairingPolicy,
    known: bool,
    code: Option<PairingCode>,
    wrong: bool,
) -> (bool, usize) {
    pair_with_grants(paths, policy, known, code, wrong, Caps::all(), false).await
}

async fn pair_with_grants(
    paths: DataPaths,
    policy: PairingPolicy,
    known: bool,
    code: Option<PairingCode>,
    wrong: bool,
    grants: Caps,
    preapproved_only: bool,
) -> (bool, usize) {
    let invitation_requested = code.is_some();
    let (client_id, _cd) = identity_for("PairClient");
    let (host_id, _hd) = identity_for("PairHost");
    let client_fp = client_id.fingerprint_hex();
    let (server, _) = make_server_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &host_id,
        PinState::new([], true),
    )
    .unwrap();
    let addr = server.local_addr().unwrap();
    let (pin_tx, pin_rx) = oneshot::channel();
    let (done_tx, done_rx) = oneshot::channel();
    let host = tokio::spawn(async move {
        let conn = RvpConnection::new(server.accept().await.unwrap().await.unwrap());
        let mut peers = PeersStore::in_memory();
        if known {
            peers
                .upsert(PeerRecord {
                    fingerprint: client_fp.clone(),
                    short_fp: client_fp[..16].into(),
                    name: "PairClient".into(),
                    trusted: true,
                    granted_caps: grants,
                    added_at_unix: 0,
                    last_connected_unix: 0,
                })
                .unwrap();
        }
        let cfg = HostConfig {
            authentication: AuthenticationSettings {
                pairing_policy: policy,
                ..Default::default()
            },
            auth_paths: Some(paths),
            audio_available: false,
            preapproved_only,
            device_name: "PairHost".into(),
            admission: AdmissionMode::AlwaysAsk,
            video_bitrate_kbps: 3000,
            video_fps: 30,
            input_sink: Some(Arc::new(removent_host::RecorderInputSink::default())),
            local_clip: None,
        };
        let result = serve_connection(
            conn.clone(),
            &host_id,
            &mut peers,
            &cfg,
            HostInteractions {
                show_pairing_pin: Box::new(move |pin| {
                    let _ = pin_tx.send(pin);
                }),
                // Generating an invitation already authorizes its one session.
                // If the host asks again, reject to catch the regression.
                admission_prompt: Box::new(move |_, _| {
                    Box::pin(async move { !invitation_requested })
                }),
            },
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
        if known {
            let stored = peers.by_fingerprint(&client_fp).unwrap();
            assert_eq!(
                stored.granted_caps, grants,
                "Pairing must preserve prior capability restrictions"
            );
            assert_eq!(stored.added_at_unix, 0);
        }
        let accepted = result.is_ok();
        if !accepted {
            conn.inner().close(0u32.into(), b"pairing rejected");
        }
        let _ = done_rx.await;
        accepted
    });
    let prompts = Arc::new(AtomicUsize::new(0));
    let count = prompts.clone();
    let (ep, _) = make_client_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &client_id,
        PinState::new([], true),
    )
    .unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        connect_session(
            ep,
            addr,
            &client_id,
            ClientConfig {
                pairing_code: code,
                device_name: "PairClient".into(),
                caps: Caps::all(),
                local_clip: None,
            },
            None,
            None,
            Some(Box::new(move |_, tx| {
                count.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    if let Ok(pin) = pin_rx.await {
                        let _ = tx.send(if wrong {
                            if pin == "000000" {
                                "000001".into()
                            } else {
                                "000000".into()
                            }
                        } else {
                            pin
                        });
                    }
                });
            })),
        ),
    )
    .await
    .expect("pairing should complete within ten seconds");
    let accepted = result.is_ok();
    let _ = done_tx.send(());
    assert_eq!(host.await.unwrap(), accepted);
    (accepted, prompts.load(Ordering::SeqCst))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repeated_pairing_preserves_grants_and_cannot_expand_login_window_access() {
    let dir = tempfile::tempdir().unwrap();
    let paths = DataPaths {
        root: dir.path().into(),
    };
    let grants = Caps {
        video: true,
        ..Caps::none()
    };
    assert_eq!(
        pair_with_grants(
            paths.clone(),
            PairingPolicy::EveryConnection,
            true,
            None,
            false,
            grants,
            false
        )
        .await,
        (true, 1)
    );
    assert_eq!(
        pair_with_grants(
            paths,
            PairingPolicy::EveryConnection,
            true,
            None,
            false,
            grants,
            true
        )
        .await,
        (false, 1)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remembered_device_skips_pairing_but_every_connection_requires_new_pin() {
    let dir = tempfile::tempdir().unwrap();
    let paths = DataPaths {
        root: dir.path().into(),
    };
    assert_eq!(
        pair(
            paths.clone(),
            PairingPolicy::RememberDevice,
            true,
            None,
            false
        )
        .await,
        (true, 0)
    );
    assert_eq!(
        pair(
            paths.clone(),
            PairingPolicy::EveryConnection,
            true,
            None,
            false
        )
        .await,
        (true, 1)
    );
    assert_eq!(
        pair(paths, PairingPolicy::EveryConnection, true, None, true).await,
        (false, 1)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invitation_authenticates_without_prompt_and_rejects_reuse_even_for_known_device() {
    let dir = tempfile::tempdir().unwrap();
    let paths = DataPaths {
        root: dir.path().into(),
    };
    let invite = Invitation::generate(&paths).unwrap();
    let wrong = PairingCode::parse(&format!(
        "{}{}",
        invite.code().locator(),
        if invite.code().secret() == "000000" {
            "000001"
        } else {
            "000000"
        }
    ))
    .unwrap();
    assert_eq!(
        pair(
            paths.clone(),
            PairingPolicy::RememberDevice,
            false,
            Some(wrong),
            false
        )
        .await,
        (false, 0)
    );
    assert!(
        Invitation::load(&paths).unwrap().is_some(),
        "wrong proof must not consume invitation"
    );
    assert_eq!(
        pair(
            paths.clone(),
            PairingPolicy::EveryConnection,
            false,
            Some(invite.code()),
            false
        )
        .await,
        (true, 0)
    );
    assert!(Invitation::load(&paths).unwrap().is_none());
    assert_eq!(
        pair(
            paths,
            PairingPolicy::RememberDevice,
            true,
            Some(invite.code()),
            false
        )
        .await,
        (false, 0)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn expired_invitation_is_rejected_before_pake() {
    let dir = tempfile::tempdir().unwrap();
    let paths = DataPaths {
        root: dir.path().into(),
    };
    let invite = Invitation::generate(&paths).unwrap();
    let file = paths.root.join("pairing-invitation.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    value["expires_at_unix"] = 0.into();
    std::fs::write(file, serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(
        pair(
            paths,
            PairingPolicy::RememberDevice,
            false,
            Some(invite.code()),
            false
        )
        .await,
        (false, 0)
    );
}
