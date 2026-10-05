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
