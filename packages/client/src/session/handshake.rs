use super::*;

/// Default negotiation wait timeout.
pub(super) const NEGOTIATE_TIMEOUT: Duration = Duration::from_secs(15);
/// SessionAccept wait timeout. On a first connect the host finishes pairing before
/// admitting the session, so the budget must cover the full pairing window (PIN
/// entry, up to 300s) plus the admission prompt (30s) plus margin (protocol.md §4.4).
pub(super) const ADMISSION_TIMEOUT: Duration = Duration::from_secs(360);

pub(super) async fn expect_msg(
    source: &mut ControlSource,
    what: &'static str,
    timeout: Duration,
    pred: impl Fn(&ControlMsg) -> bool + Copy,
) -> Result<ControlMsg, ConnectError> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let item = tokio::time::timeout_at(deadline, source.next())
            .await
            .map_err(|_| ConnectError::Timeout(what))?
            .transpose()
            .map_err(ConnectError::Net)?
            .ok_or(ConnectError::Timeout(what))?;
        if let ControlItem::Msg(m) = item
            && pred(&m)
        {
            return Ok(*m);
        }
    }
}

/// Establish a full session. When `resume_token` is Some, takes the quick-resume path
/// (no PIN input needed) — but only if the host explicitly accepted the token;
/// a rejected token falls back to full negotiation.
#[allow(clippy::too_many_arguments)]
pub async fn connect_session(
    ep: removent_net::quinn::Endpoint,
    addr: SocketAddr,
    identity: &DeviceIdentity,
    cfg: ClientConfig,
    resume_token: Option<[u8; 16]>,
    prev_ack: Option<NegotiateAck>,
    pin_request: Option<PinRequest>,
) -> Result<ClientSession, ConnectError> {
    connect_session_with_confirmation(
        ep,
        addr,
        identity,
        cfg,
        resume_token,
        prev_ack,
        pin_request,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn connect_session_with_confirmation(
    ep: removent_net::quinn::Endpoint,
    addr: SocketAddr,
    identity: &DeviceIdentity,
    cfg: ClientConfig,
    resume_token: Option<[u8; 16]>,
    prev_ack: Option<NegotiateAck>,
    pin_request: Option<PinRequest>,
    confirmation: Option<removent_net::CertificateConfirmation>,
) -> Result<ClientSession, ConnectError> {
    // QUIC handshake (briefly retries while the server is not ready).
    let conn = if let Some(confirmation) = confirmation {
        RvpConnection::new(
            removent_net::confirm_quic_peer(&ep, addr, "removent", confirmation).await?,
        )
    } else {
        let mut attempt = 0;
        loop {
            match ep.connect(addr, "removent") {
                Ok(connecting) => {
                    let qconn = connecting
                        .await
                        .map_err(|e| ConnectError::Rejected(format!("quinn: {e}")))?;
                    break removent_net::RvpConnection::new(qconn);
                }
                Err(e) => {
                    attempt += 1;
                    if attempt > 50 {
                        return Err(ConnectError::Rejected(format!("connect: {e}")));
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
        }
    };

    let peer_fp_full = conn
        .peer_fingerprint()
        .map(hex_encode)
        .ok_or_else(|| ConnectError::Rejected("peer fingerprint missing".into()))?;

    let (server_ack, sink, mut source) = conn
        .connect_handshake(HandshakeClient {
            magic: removent_proto::MAGIC,
            proto_version: removent_proto::PROTO_VERSION,
            // Honest declaration: file-transfer/two-way-audio are both unimplemented.
            feature_bits: removent_proto::feature_bits::SOFTWARE_AV1
                | removent_proto::feature_bits::AUTH_METHODS
                | if cfg.pairing_code.is_some() {
                    removent_proto::feature_bits::PAIRING_INVITATION
                } else {
                    0
                },
            hello: Hello {
                app_version: removent_core::APP_VERSION.to_string(),
                device_name: cfg.device_name.clone(),
                os_version: platform_version(),
                caps: cfg.caps,
                resume_token,
            },
        })
        .await?;

    // The quick-resume fast path is taken only when the host explicitly accepted
    // the token; a rejected/expired token falls back to full negotiation below.
    let resume_accepted = cfg.pairing_code.is_none()
        && resume_token.is_some()
        && server_ack.resume_accepted == Some(true);

    // First connect: start the pairing-initiator task concurrently (the host accepts
    // when needed) — but only when the host does not know us yet; a trusted peer
    // never pairs, so the user is never asked for a PIN (no popup flash).
    // The pairing result comes back over a oneshot, with errors attributed to Pairing
    // rather than a bare Timeout.
    let mut pairing_rx: Option<oneshot::Receiver<Result<(), String>>> = None;
    let mut pairing_task: Option<AbortOnDrop> = None;
    if !resume_accepted && (!server_ack.peer_known || cfg.pairing_code.is_some()) {
        let pin_request = pin_request
            .or_else(|| {
                cfg.pairing_code
                    .as_ref()
                    .map(|_| Box::new(|_, _| {}) as PinRequest)
            })
            .ok_or_else(|| {
                ConnectError::Pairing(
                    "Authentication required; reconnect and enter the host credential".into(),
                )
            })?;
        let (handle, rx) = spawn_pairing_initiator(
            conn.clone(),
            identity,
            identity.fingerprint_hex(),
            peer_fp_full,
            pin_request,
            cfg.pairing_code.clone(),
        );
        pairing_rx = Some(rx);
        pairing_task = Some(AbortOnDrop(handle));
    }

    let mut sink = sink;
    sink.send(ControlMsg::SessionRequest { caps: cfg.caps })
        .await?;

    // On failure paths, collect the pairing result: briefly wait for the pairing task
    // to produce an outcome (on a wrong PIN the host rejects pairing before
    // disconnecting, so the result is available almost immediately), avoiding the user
    // seeing only a bare Timeout.
    async fn take_pairing_error(
        rx: &mut Option<oneshot::Receiver<Result<(), String>>>,
    ) -> Option<String> {
        let rx = rx.take()?;
        match tokio::time::timeout(Duration::from_millis(500), rx).await {
            Ok(Ok(Err(pe))) => Some(pe),
            _ => None,
        }
    }

    // SessionAccept wait: first-time pairing (up to 300s) + host prompt 30s + margin.
    let response = {
        let wait = expect_msg(&mut source, "SessionAccept", ADMISSION_TIMEOUT, |m| {
            matches!(
                m,
                ControlMsg::SessionAccept | ControlMsg::SessionReject { .. }
            )
        });
        tokio::pin!(wait);
        if let Some(rx) = pairing_rx.as_mut() {
            tokio::select! {
                result = &mut wait => result,
                result = rx => {
                    pairing_rx = None;
                    match result {
                        Ok(Ok(())) => wait.await,
                        Ok(Err(error)) => return Err(ConnectError::Pairing(error)),
                        Err(_) => return Err(ConnectError::Pairing("Authentication was cancelled".into())),
                    }
                }
            }
        } else {
            wait.await
        }
    };
    let resp = match response {
        Ok(m) => m,
        Err(e) => {
            // Prefer surfacing the pairing failure reason (wrong PIN, etc.) over
            // reporting a bare Timeout.
            if let Some(pe) = take_pairing_error(&mut pairing_rx).await {
                return Err(ConnectError::Pairing(pe));
            }
            return Err(e);
        }
    };
    match resp {
        ControlMsg::SessionReject { reason } => {
            // Busy is a definitive host verdict: report it directly. The host drops
            // the connection right after sending the reject, so the pairing stream's
            // read error ("application closed") would otherwise win the 500ms race in
            // take_pairing_error and mask the real reason.
            if !matches!(reason, removent_proto::RejectReason::Busy)
                && let Some(pe) = take_pairing_error(&mut pairing_rx).await
            {
                return Err(ConnectError::Pairing(pe));
            }
            return Err(ConnectError::Rejected(format!("{reason:?}")));
        }
        // Quick resume: only when the host accepted the token — it then skips
        // negotiation and we reuse the previous parameters. A rejected token
        // falls through to full negotiation so both ends stay in sync.
        ControlMsg::SessionAccept if resume_accepted && prev_ack.is_some() => {
            return build_session(conn, sink, source, prev_ack.unwrap(), cfg);
        }
        ControlMsg::SessionAccept => {}
        _ => unreachable!(),
    }

    // SessionAccept is not proof of authentication. Whenever pairing started,
    // require its cryptographic confirmation before accepting media or input.
    if let Some(rx) = pairing_rx.take() {
        tokio::time::timeout(Duration::from_secs(15), rx)
            .await
            .map_err(|_| ConnectError::Timeout("authentication confirmation"))?
            .map_err(|_| ConnectError::Pairing("Authentication was cancelled".into()))?
            .map_err(ConnectError::Pairing)?;
    }
    // Abort on success, early error, and cancellation of connect_session.
    drop(pairing_task.take());

    let offer = expect_msg(&mut source, "NegotiateOffer", NEGOTIATE_TIMEOUT, |m| {
        matches!(m, ControlMsg::NegotiateOffer { .. })
    })
    .await?;
    let ControlMsg::NegotiateOffer { n } = offer else {
        unreachable!()
    };
    let n = *n;

    // The client accepts the host's proposal, trimmed to the declared capabilities:
    // without the audio cap nothing would consume the decoded PCM (§5.2).
    sink.send(ControlMsg::NegotiateReply {
        ack: Box::new(NegotiateAck {
            video: n.video,
            audio: removent_proto::AudioParams {
                enabled: cfg.caps.audio && n.audio.enabled,
                ..n.audio
            },
            resume_token: None,
        }),
    })
    .await?;

    let final_ack = expect_msg(
        &mut source,
        "final NegotiateReply",
        NEGOTIATE_TIMEOUT,
        |m| matches!(m, ControlMsg::NegotiateReply { .. }),
    )
    .await?;
    let ControlMsg::NegotiateReply { ack } = final_ack else {
        unreachable!()
    };
    let ack = *ack;

    build_session(conn, sink, source, ack, cfg)
}
