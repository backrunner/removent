use super::*;

pub(super) fn hex_encode(bytes: [u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(target_os = "ios")]
pub(super) fn platform_version() -> String {
    "iOS".into()
}

/// Desktop version is queried once; iOS cannot spawn subprocesses.
#[cfg(not(target_os = "ios"))]
pub(super) fn platform_version() -> String {
    static V: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let version = V.get_or_init(|| {
        std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".into())
    });
    format!("macOS {version}")
}

/// Quick resume within the 30s window after a disconnect (protocol.md §7.3/§7.4).
/// `prev_ack` is the previous session's `negotiated` parameters; after a successful
/// resume, `session.resume_token` is updated with the host's rotated new token.
pub async fn quick_resume(
    ep: removent_net::quinn::Endpoint,
    addr: SocketAddr,
    identity: &DeviceIdentity,
    cfg: ClientConfig,
    token: [u8; 16],
    prev_ack: NegotiateAck,
) -> Result<ClientSession, ConnectError> {
    connect_session(ep, addr, identity, cfg, Some(token), Some(prev_ack), None).await
}

// ---------------- pairing initiation ----------------

/// Returns (task handle, result channel). Pairing errors are reported over the channel
/// and attributed to Pairing by the connect flow.
pub(super) fn spawn_pairing_initiator(
    conn: RvpConnection,
    identity: &DeviceIdentity,
    fp_self_full: String,
    fp_peer_full: String,
    pin_request: PinRequest,
    invitation: Option<removent_core::pairing_invitation::PairingCode>,
    host_identity_verified: bool,
) -> (
    tokio::task::JoinHandle<()>,
    oneshot::Receiver<Result<(), String>>,
) {
    let identity = identity.clone();
    let (tx, rx) = oneshot::channel();
    let handle = tokio::spawn(async move {
        let result = run_pairing(
            conn,
            &identity,
            &fp_self_full,
            &fp_peer_full,
            pin_request,
            invitation,
            host_identity_verified,
        )
        .await;
        match &result {
            Ok(()) => tracing::info!("pairing completed"),
            Err(e) => tracing::warn!(err=%e, "pairing failed"),
        }
        let _ = tx.send(result.map_err(|e| e.to_string()));
    });
    (handle, rx)
}

pub(super) async fn run_pairing(
    conn: RvpConnection,
    identity: &DeviceIdentity,
    fp_self_full: &str,
    fp_peer_full: &str,
    pin_request: PinRequest,
    invitation: Option<removent_core::pairing_invitation::PairingCode>,
    host_identity_verified: bool,
) -> Result<(), ConnectError> {
    let (mut psink, mut psource) = conn.open_pairing().await?;

    let (begin_msg, nonce_c) = client_begin(fp_self_full);
    let begin_msg = if let Some(code) = &invitation {
        PairingMsg::BeginInvitation {
            nonce_c,
            fp_c: fp_self_full.into(),
            locator: code.locator().into(),
        }
    } else {
        begin_msg
    };
    psink.send(begin_msg).await.map_err(ConnectError::Net)?;

    let challenge = tokio::time::timeout(Duration::from_secs(15), psource.next())
        .await
        .map_err(|_| ConnectError::Timeout("pairing challenge"))?
        .transpose()
        .map_err(ConnectError::Net)?
        .ok_or(ConnectError::Timeout("pairing challenge"))?;
    if let PairingMsg::SystemChallenge { nonce_h } = challenge {
        if !host_identity_verified || invitation.is_some() {
            return Err(ConnectError::Pairing(
                "Confirm the host certificate before using a system account".into(),
            ));
        }
        let (tx, rx) = oneshot::channel();
        pin_request(removent_proto::AuthenticationMode::SystemAccount, tx);
        let value = tokio::time::timeout(Duration::from_secs(300), rx)
            .await
            .map_err(|_| ConnectError::Timeout("system account input"))?
            .map_err(|_| ConnectError::Pairing("authentication cancelled".into()))?;
        let credentials = removent_proto::SystemCredentials::decode(&value)
            .ok_or_else(|| ConnectError::Pairing("Invalid system username or password".into()))?;
        drop(value);
        psink
            .send(PairingMsg::SystemVerify {
                nonce_c,
                nonce_h,
                credentials,
            })
            .await?;
        let confirm = tokio::time::timeout(Duration::from_secs(30), psource.next())
            .await
            .map_err(|_| ConnectError::Timeout("system account verification"))?
            .transpose()?
            .ok_or(ConnectError::Timeout("system account verification"))?;
        return match confirm {
            PairingMsg::SystemConfirm { nonce_c: c, nonce_h: h, ok: true }
                if c == nonce_c && h == nonce_h => Ok(()),
            _ => Err(ConnectError::Pairing("System account authentication failed; check the sharing user's username and login password".into())),
        };
    }
    let (mode, challenges) = match &challenge {
        PairingMsg::Challenge { .. } => (
            removent_proto::AuthenticationMode::PairingCode,
            vec![challenge.clone()],
        ),
        PairingMsg::AuthChallenge { mode, challenges }
            if matches!(
                mode,
                removent_proto::AuthenticationMode::Password
                    | removent_proto::AuthenticationMode::Otp
            ) && !challenges.is_empty()
                && challenges.len() <= 3 =>
        {
            (
                *mode,
                challenges.iter().cloned().map(PairingMsg::from).collect(),
            )
        }
        _ => {
            return Err(ConnectError::Pairing(
                "expected authentication challenge".into(),
            ));
        }
    };
    let (pin_tx, pin_rx) = oneshot::channel();
    if let Some(code) = invitation {
        if mode != removent_proto::AuthenticationMode::PairingCode {
            return Err(ConnectError::Pairing(
                "Host no longer accepts pairing invitations".into(),
            ));
        }
        let _ = pin_tx.send(code.secret().into());
    } else {
        pin_request(mode, pin_tx);
    }
    let pin = tokio::time::timeout(Duration::from_secs(300), pin_rx)
        .await
        .map_err(|_| ConnectError::Timeout("authentication input"))?
        .map_err(|_| ConnectError::Pairing("authentication cancelled".into()))?;
    if !mode.valid_input(&pin) {
        return Err(ConnectError::Pairing("invalid authentication input".into()));
    }
    let mut proofs = Vec::new();
    let mut shared_keys = Vec::new();
    for challenge in &challenges {
        let (proof, shared) = client_verify(
            &nonce_c,
            challenge,
            fp_self_full,
            fp_peer_full,
            &pin,
            identity,
        )?;
        proofs.push(proof);
        shared_keys.push(shared);
    }
    let verify = if mode == removent_proto::AuthenticationMode::PairingCode {
        proofs.remove(0)
    } else {
        PairingMsg::AuthVerify {
            proofs: proofs
                .into_iter()
                .map(|proof| {
                    let PairingMsg::Verify {
                        msg_c,
                        confirm_c,
                        vk_c,
                        sig_c,
                    } = proof
                    else {
                        unreachable!()
                    };
                    removent_net::PairingProof {
                        msg_c,
                        confirm_c,
                        vk_c,
                        sig_c,
                    }
                })
                .collect(),
        }
    };
    psink.send(verify).await.map_err(ConnectError::Net)?;

    let confirm = tokio::time::timeout(Duration::from_secs(15), psource.next())
        .await
        .map_err(|_| ConnectError::Timeout("pairing confirm"))?
        .transpose()
        .map_err(ConnectError::Net)?
        .ok_or(ConnectError::Timeout("pairing confirm"))?;
    let peer_vk = peer_verifying_key(&conn)
        .ok_or_else(|| ConnectError::Pairing("peer certificate key extract failed".into()))?;
    let valid = challenges
        .iter()
        .zip(shared_keys)
        .any(|(challenge, shared)| {
            let PairingMsg::Challenge { nonce_h, .. } = challenge else {
                return false;
            };
            client_confirm_check(
                &shared,
                &nonce_c,
                nonce_h,
                fp_self_full,
                fp_peer_full,
                &confirm,
                &peer_vk,
            )
            .is_ok()
        });
    if !valid {
        return Err(ConnectError::Pairing(
            "authentication failed; check password or code".into(),
        ));
    }
    Ok(())
}

/// Extract the long-term public key from the peer's TLS certificate (self-signed
/// ed25519) for pairing signature verification.
pub(super) fn peer_verifying_key(conn: &RvpConnection) -> Option<ed25519_dalek::VerifyingKey> {
    let chain = conn
        .inner()
        .peer_identity()?
        .downcast::<Vec<removent_net::quinn::rustls::pki_types::CertificateDer<'static>>>()
        .ok()?;
    let der = chain.first()?;
    // Ed25519 SubjectPublicKeyInfo fixed prefix, followed by the 32-byte public key.
    const SPKI_PREFIX: [u8; 12] = [
        0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
    ];
    let bytes: &[u8] = der.as_ref();
    let pos = bytes
        .windows(SPKI_PREFIX.len())
        .position(|w| w == SPKI_PREFIX)?;
    let key: [u8; 32] = bytes
        .get(pos + SPKI_PREFIX.len()..pos + SPKI_PREFIX.len() + 32)?
        .try_into()
        .ok()?;
    ed25519_dalek::VerifyingKey::from_bytes(&key).ok()
}
// ---------------- media receive ----------------

#[cfg(test)]
mod system_account_tests {
    use super::*;

    #[tokio::test]
    async fn system_credentials_require_host_trust_and_a_matching_success_confirmation() {
        for (trusted, accepted, replay, expected) in [
            (true, true, false, true),
            (false, true, false, false),
            (true, false, false, false),
            (true, true, true, false),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let identity = removent_core::identity::load_or_create(
                &removent_core::DataPaths {
                    root: dir.path().into(),
                },
                "auth-fixture",
            )
            .unwrap();
            let (host, _) = removent_net::make_server_endpoint(
                "127.0.0.1:0".parse().unwrap(),
                &identity,
                removent_net::PinState::new([], true),
            )
            .unwrap();
            let (client, _) = removent_net::make_client_endpoint(
                "127.0.0.1:0".parse().unwrap(),
                &identity,
                removent_net::PinState::new([identity.fingerprint], false),
            )
            .unwrap();
            let (c, h) = tokio::join!(
                client
                    .connect(host.local_addr().unwrap(), "removent")
                    .unwrap(),
                async { host.accept().await.unwrap().await }
            );
            let client_conn = RvpConnection::new(c.unwrap());
            let host_conn = RvpConnection::new(h.unwrap());
            let server = host_conn.clone();
            let host_task = tokio::spawn(async move {
                let (mut sink, mut source) = server.accept_pairing().await.unwrap();
                let PairingMsg::Begin { nonce_c, .. } = source.next().await.unwrap().unwrap()
                else {
                    panic!("begin");
                };
                let nonce_h = [7; 16];
                sink.send(PairingMsg::SystemChallenge { nonce_h })
                    .await
                    .unwrap();
                if !trusted {
                    return;
                }
                let PairingMsg::SystemVerify {
                    nonce_c: c,
                    nonce_h: h,
                    credentials,
                } = source.next().await.unwrap().unwrap()
                else {
                    panic!("system credentials");
                };
                assert_eq!((c, h), (nonce_c, nonce_h));
                assert_eq!(credentials.username, "owner");
                assert_eq!(credentials.password, "test-password");
                sink.send(PairingMsg::SystemConfirm {
                    nonce_c,
                    nonce_h: if replay { [0; 16] } else { nonce_h },
                    ok: accepted,
                })
                .await
                .unwrap();
            });
            let result = tokio::time::timeout(
                Duration::from_secs(5),
                run_pairing(
                    client_conn,
                    &identity,
                    &identity.fingerprint_hex(),
                    &identity.fingerprint_hex(),
                    Box::new(move |mode, tx| {
                        assert!(
                            trusted,
                            "unconfirmed hosts must not even request credentials"
                        );
                        assert_eq!(mode, removent_proto::AuthenticationMode::SystemAccount);
                        tx.send(
                            removent_proto::SystemCredentials {
                                username: "owner".into(),
                                password: "test-password".into(),
                            }
                            .encode(),
                        )
                        .unwrap();
                    }),
                    None,
                    trusted,
                ),
            )
            .await
            .unwrap();
            assert_eq!(result.is_ok(), expected);
            host_task.await.unwrap();
        }
    }
}
