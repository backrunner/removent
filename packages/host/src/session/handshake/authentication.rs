use super::*;

/// Password and TOTP use the same PAKE identity/transcript binding as pairing.
/// TOTP offers three independently blinded candidates for +/- one 30s clock step.
pub(super) async fn run_authentication(
    conn: &RvpConnection,
    identity: &DeviceIdentity,
    peer_fp: &str,
    cfg: &HostConfig,
) -> Result<(), HostError> {
    use removent_core::AuthenticationMode;
    use removent_net::PairingMsg;
    let (mut sink, mut source) =
        tokio::time::timeout(Duration::from_secs(15), conn.accept_pairing())
            .await
            .map_err(|_| HostError::Timeout("authentication stream"))??;
    let begin = tokio::time::timeout(Duration::from_secs(15), source.next())
        .await
        .map_err(|_| HostError::Timeout("authentication begin"))?
        .transpose()?
        .ok_or(HostError::Timeout("authentication begin"))?;
    // Host-wide bucket prevents bypass by generating fresh device identities.
    if !pairing_begin_limiter().lock().unwrap().allow(
        &format!("auth:{}", identity.fingerprint_hex()),
        std::time::Instant::now(),
    ) {
        return Err(HostError::Rejected(
            "Too many authentication attempts; retry in five seconds".into(),
        ));
    }
    let step = now_unix() / 30;
    if cfg.authentication.mode == AuthenticationMode::SystemAccount {
        return system_account::exchange(
            begin,
            peer_fp,
            &mut sink,
            &mut source,
            system_account::verify,
        )
        .await;
    }

    let candidates = if cfg.authentication.mode == AuthenticationMode::Password {
        vec![(0, cfg.authentication.password.clone())]
    } else {
        let totp = cfg.authentication.totp().map_err(HostError::Rejected)?;
        [step.saturating_sub(1), step, step + 1]
            .into_iter()
            .map(|step| (step, totp.generate(step * 30).to_string()))
            .collect()
    };
    let mut states = Vec::new();
    for (step, secret) in candidates {
        states.push((
            step,
            removent_net::host_on_begin_with_secret(&begin, &secret)?,
        ));
    }
    sink.send(PairingMsg::AuthChallenge {
        mode: cfg.authentication.mode,
        challenges: states
            .iter()
            .map(|(_, hs)| {
                let PairingMsg::Challenge { nonce_h, msg_h } = &hs.reply else {
                    unreachable!()
                };
                removent_net::PairingChallenge {
                    nonce_h: *nonce_h,
                    msg_h: msg_h.clone(),
                }
            })
            .collect(),
    })
    .await?;
    let verify = tokio::time::timeout(Duration::from_secs(300), source.next())
        .await
        .map_err(|_| HostError::Timeout("authentication input"))?
        .transpose()?
        .ok_or(HostError::Timeout("authentication input"))?;
    let PairingMsg::AuthVerify { proofs } = verify else {
        return Err(HostError::Rejected("Expected authentication proof".into()));
    };
    if proofs.len() != states.len() {
        return Err(HostError::Rejected(
            "Invalid authentication proof count".into(),
        ));
    }
    for ((step, hs), proof) in states.into_iter().zip(proofs) {
        let (confirm, shared) = removent_net::host_verify(hs, &proof.into(), identity, peer_fp)?;
        if shared.is_some() {
            if cfg.authentication.mode == AuthenticationMode::Otp {
                if step.abs_diff(now_unix() / 30) > 1 {
                    break;
                }
                let paths = cfg.auth_paths.as_ref().ok_or_else(|| {
                    HostError::Rejected("OTP replay storage is unavailable".into())
                })?;
                cfg.authentication
                    .consume_otp(paths, step)
                    .map_err(HostError::Rejected)?;
            }
            sink.send(confirm).await?;
            return Ok(());
        }
    }
    sink.send(PairingMsg::Confirm {
        ok: false,
        confirm_h: [0; 32],
        sig_h: vec![],
    })
    .await?;
    Err(HostError::Rejected("Authentication failed".into()))
}
