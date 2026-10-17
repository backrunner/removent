use super::*;
use removent_core::{AuthenticationMode as Mode, AuthenticationSettings, PeerRecord};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deny_all_rejects_before_any_credential_prompt_or_otp_consumption() {
    for mode in [
        Mode::SystemAccount,
        Mode::PairingCode,
        Mode::Password,
        Mode::Otp,
        Mode::None,
    ] {
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
    attempt_over(auth, input, known, admission, paths, None).await
}

async fn attempt_over(
    auth: AuthenticationSettings,
    input: Option<String>,
    known: bool,
    admission: AdmissionMode,
    paths: DataPaths,
    websocket: Option<bool>,
) -> (bool, usize) {
    let (client_id, _cd) = identity_for("AuthClient");
    let (host_id, _hd) = identity_for("AuthHost");
    let client_fp = client_id.fingerprint_hex();
    let host_fingerprint = host_id.fingerprint;
    let (server, _) = make_server_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &host_id,
        PinState::new([], true),
    )
    .unwrap();
    let mut address = server.local_addr().unwrap();
    let mut relay_resources = None;
    if let Some(websocket) = websocket {
        use removent_relay::config::{Room, ServerConfig, TunnelConfig, token_hash};
        let (relay_id, relay_dir) = identity_for("AuthRelay");
        let relay_ep =
            removent_relay::server_endpoint("127.0.0.1:0".parse().unwrap(), &relay_id).unwrap();
        let listener = if websocket {
            Some(tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap())
        } else {
            None
        };
        let relay_address = listener
            .as_ref()
            .map(|l| l.local_addr().unwrap())
            .unwrap_or(relay_ep.local_addr().unwrap());
        let host_token = "11".repeat(32);
        let client_token = "22".repeat(32);
        let config = ServerConfig {
            updates: Default::default(),
            listen: relay_address,
            allowed_cidrs: vec![],
            identity_dir: relay_dir.path().into(),
            max_connections: 16,
            max_clients_per_room: 4,
            max_bytes_per_second: 1_000_000_000,
            rooms: vec![Room {
                name: "auth-matrix".into(),
                host_token_sha256: hex::encode(token_hash(&host_token).unwrap()),
                client_token_sha256: hex::encode(token_hash(&client_token).unwrap()),
                host_public_keys: vec![hex::encode(host_id.verifying_key().as_bytes())],
                client_public_keys: vec![hex::encode(client_id.verifying_key().as_bytes())],
            }],
        };
        let stop = tokio_util::sync::CancellationToken::new();
        let relay = if let Some(listener) = listener {
            tokio::spawn(removent_relay::websocket::serve(
                listener, config, stop, None,
            ))
        } else {
            tokio::spawn(removent_relay::server::serve(relay_ep, config, stop))
        };
        let tunnel = TunnelConfig {
            server: format!("removent://{relay_address}"),
            transport: if websocket {
                removent_core::removent_uri::RelayTransport::WebSocket
            } else {
                removent_core::removent_uri::RelayTransport::Quic
            },
            insecure_loopback: websocket,
            server_fingerprint: if websocket {
                String::new()
            } else {
                relay_id.fingerprint_hex()
            },
            host_fingerprint: host_id.fingerprint_hex(),
            room: "auth-matrix".into(),
            token: host_token,
            server_name: String::new(),
            accept_invalid_certificate: false,
        };
        let host_tunnel = removent_relay::client::connect_host(&tunnel, &host_id)
            .await
            .unwrap();
        let forwarding = tokio::spawn(host_tunnel.run(address));
        let bridge = removent_relay::client::ClientBridge::start(
            &TunnelConfig {
                token: client_token,
                ..tunnel
            },
            &client_id,
        )
        .await
        .unwrap();
        address = bridge.address;
        relay_resources = Some((relay_dir, AbortTask(relay), AbortTask(forwarding), bridge));
    }
    let mode = auth.mode;
    let pairing_pin = Arc::new(std::sync::Mutex::new(String::new()));
    let host_pin = pairing_pin.clone();
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
            show_pairing_pin: Box::new(move |pin| {
                assert_eq!(
                    mode,
                    Mode::PairingCode,
                    "unattended modes must not display a pairing PIN"
                );
                *host_pin.lock().unwrap() = pin;
            }),
            admission_prompt: Box::new(move |_, _| {
                assert_eq!(
                    mode,
                    Mode::PairingCode,
                    "unattended modes must not ask for local approval"
                );
                Box::pin(async { true })
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
            usize::from(known || (mode == Mode::PairingCode && accepted)),
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
        PinState::new([host_fingerprint], false),
    )
    .unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        connect_session(
            endpoint,
            address,
            &client_id,
            ClientConfig {
                video_quality: Default::default(),
                host_identity_verified: true,
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
                if mode == Mode::PairingCode {
                    let _ = tx.send(pairing_pin.lock().unwrap().clone());
                } else if let Some(input) = input {
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
    drop(relay_resources);
    (accepted, prompts.load(Ordering::SeqCst))
}

struct AbortTask(tokio::task::JoinHandle<anyhow::Result<()>>);
impl Drop for AbortTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn all_host_authentication_modes_negotiate_through_both_relay_carriers() {
    for websocket in [false, true] {
        for mode in [Mode::PairingCode, Mode::Password, Mode::Otp, Mode::None] {
            let dir = tempfile::tempdir().unwrap();
            let auth = AuthenticationSettings {
                mode,
                password: "host password differs from relay credential".into(),
                otp_secret: AuthenticationSettings::generate_otp_secret(),
                ..Default::default()
            };
            let input = match mode {
                Mode::Password => Some(auth.password.clone()),
                Mode::Otp => Some(auth.totp().unwrap().generate_current().to_string()),
                _ => None,
            };
            assert_eq!(
                attempt_over(
                    auth.clone(),
                    input,
                    false,
                    AdmissionMode::AlwaysAsk,
                    DataPaths {
                        root: dir.path().into()
                    },
                    Some(websocket)
                )
                .await,
                (true, usize::from(mode != Mode::None)),
                "mode={mode:?}, websocket={websocket}"
            );
            if matches!(mode, Mode::Password | Mode::Otp) {
                let wrong = if mode == Mode::Password {
                    "22".repeat(32)
                } else {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs();
                    let totp = auth.totp().unwrap();
                    let candidates: Vec<_> = [now.saturating_sub(30), now, now + 30]
                        .into_iter()
                        .map(|time| totp.generate(time).to_string())
                        .collect();
                    (0..1_000_000)
                        .map(|n| format!("{n:06}"))
                        .find(|code| !candidates.contains(code))
                        .unwrap()
                };
                assert_eq!(
                    attempt_over(
                        auth,
                        Some(wrong),
                        false,
                        AdmissionMode::AlwaysAsk,
                        DataPaths {
                            root: dir.path().into()
                        },
                        Some(websocket)
                    )
                    .await,
                    (false, 1)
                );
            }
        }
    }
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
    let token = totp.generate(now).to_string();
    let candidates: Vec<_> = [now.saturating_sub(30), now, now + 30]
        .into_iter()
        .map(|time| totp.generate(time).to_string())
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
            Some(totp.generate(now.saturating_sub(120)).to_string()),
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn system_account_rejects_other_users_and_cancel_without_pin_fallback() {
    for carrier in [None, Some(false), Some(true)] {
        let directory = tempfile::tempdir().unwrap();
        let paths = DataPaths {
            root: directory.path().into(),
        };
        let input = removent_proto::SystemCredentials {
            username: "root".into(),
            password: "synthetic-password".into(),
        }
        .encode();
        assert_eq!(
            attempt_over(
                AuthenticationSettings::default(),
                Some(input),
                true,
                AdmissionMode::AlwaysAsk,
                paths,
                carrier
            )
            .await,
            (false, 1)
        );
    }
    let directory = tempfile::tempdir().unwrap();
    assert_eq!(
        attempt(
            AuthenticationSettings::default(),
            None,
            false,
            AdmissionMode::AlwaysAsk,
            DataPaths {
                root: directory.path().into()
            }
        )
        .await,
        (false, 1)
    );
}
