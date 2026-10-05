use super::*;

pub(super) async fn expect_msg(
    source: &mut ControlSource,
    what: &'static str,
    pred: impl Fn(&ControlMsg) -> bool,
) -> Result<ControlMsg, HostError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let item = tokio::time::timeout_at(deadline, source.next())
            .await
            .map_err(|_| HostError::Timeout(what))?
            .transpose()
            .map_err(HostError::Net)?
            .ok_or(HostError::Timeout(what))?;
        match item {
            ControlItem::Msg(m) => {
                if pred(&m) {
                    return Ok(*m);
                }
            }
            ControlItem::Skipped => continue,
        }
    }
}

/// Serve one inbound connection through negotiation and return the session handle;
/// the caller then spawns [`spawn_video_loop`] / [`spawn_audio_loop`] / the control-pump task.
pub async fn serve_connection(
    conn: RvpConnection,
    identity: &DeviceIdentity,
    peers: &mut PeersStore,
    cfg: &HostConfig,
    interactions: HostInteractions,
    main_display: removent_proto::DisplayInfo,
) -> Result<
    (
        EstablishedSession,
        mpsc::Receiver<()>,
        tokio::sync::watch::Receiver<QualityState>,
        mpsc::Receiver<ControlMsg>,
        ControlSink,
        ControlSource,
    ),
    HostError,
> {
    cfg.authentication.validate().map_err(HostError::Rejected)?;
    let peer_fp = conn
        .peer_fingerprint()
        .map(hex::encode)
        .ok_or_else(|| HostError::Rejected("peer fingerprint missing".into()))?;
    if cfg.preapproved_only && !peers.by_fingerprint(&peer_fp).is_some_and(|p| p.trusted) {
        return Err(HostError::Rejected(
            "previously trusted devices only".into(),
        ));
    }

    // The resume token is validated exactly once, inside the handshake closure, and
    // the verdict is reused for the branch below: two independent validations could
    // straddle the 30s window boundary and desynchronise the two ends (the host
    // taking the resume path while the client was told resume_accepted=false, or
    // vice versa).
    let resume_verdict: std::sync::Mutex<Option<(NegotiateAck, Caps, bool)>> =
        std::sync::Mutex::new(None);
    let (hello, mut sink, mut source) = conn
        .accept_handshake(|hello| {
            let verdict = hello
                .hello
                .resume_token
                .as_ref()
                .and_then(|t| validate_resume_full(&peer_fp, t))
                .filter(|(_, caps, _)| {
                    cfg.admission != AdmissionMode::DenyAll
                        && (cfg.authentication.mode
                            != removent_core::AuthenticationMode::PairingCode
                            || cfg.authentication.pairing_policy
                                != removent_core::authentication::PairingPolicy::EveryConnection)
                        && hello.feature_bits & removent_proto::feature_bits::PAIRING_INVITATION
                            == 0
                        && resume_policy_matches(&peer_fp, &cfg.authentication)
                        && (!(cfg.preapproved_only
                            || cfg.authentication.mode
                                == removent_core::AuthenticationMode::PairingCode)
                            || decide(cfg.admission, peers, &peer_fp, cfg.available_caps(*caps))
                                == AdmissionDecision::Allow)
                });
            let accepted = verdict.is_some();
            *resume_verdict.lock().unwrap() = verdict;
            HandshakeServer {
                proto_version: PROTO_VERSION,
                // File-transfer/two-way-audio/hdr remain unimplemented; the
                // software AV1 codec is available on every build and is still
                // opt-in during negotiation.
                feature_bits: removent_proto::feature_bits::SOFTWARE_AV1
                    | removent_proto::feature_bits::AUTH_METHODS,
                device_name: cfg.device_name.clone(),
                // Honest resume verdict (§7.4): Some(true) only when the presented token
                // validated; Some(false) tells the client to fall back to full
                // negotiation instead of desynchronising the two ends.
                resume_accepted: hello.hello.resume_token.as_ref().map(|_| accepted),
                // A trusted client skips pairing (and its PIN prompt) entirely.
                peer_known: cfg.admission == AdmissionMode::DenyAll
                    || match cfg.authentication.mode {
                        removent_core::AuthenticationMode::PairingCode => {
                            cfg.authentication.pairing_policy
                                == removent_core::authentication::PairingPolicy::RememberDevice
                                && hello.feature_bits
                                    & removent_proto::feature_bits::PAIRING_INVITATION
                                    == 0
                                && peers.by_fingerprint(&peer_fp).is_some_and(|p| p.trusted)
                        }
                        removent_core::AuthenticationMode::None => true,
                        _ => false,
                    },
            }
        })
        .await?;

    if cfg.admission == AdmissionMode::DenyAll
        || (hello.feature_bits & removent_proto::feature_bits::PAIRING_INVITATION != 0
            && cfg.authentication.mode != removent_core::AuthenticationMode::PairingCode)
    {
        sink.send(ControlMsg::SessionReject {
            reason: removent_proto::RejectReason::Denied,
        })
        .await?;
        return Err(HostError::Rejected(
            "Connections are disabled or invitation authentication is unavailable".into(),
        ));
    }

    // Quick-resume path: skips pairing/admission and reuses the last negotiation
    // parameters; the old token is single-use and rotated after consumption (§7.4).
    if let Some(token) = hello.hello.resume_token
        && let Some((prev_ack, peer_caps, matched_prev)) = resume_verdict.into_inner().unwrap()
    {
        invalidate_resume(&peer_fp);
        let new_token = new_token();
        let peer_caps = cfg.available_caps(peer_caps);
        let mut ack = prev_ack;
        ack.audio.enabled &= peer_caps.audio;
        ack.resume_token = Some(new_token);
        // Tolerate one lost rotation reply: the just-consumed current token stays
        // valid once more as the previous generation, so a client that never
        // received the new token can still resume; a consumed previous-generation
        // token is retired for good.
        let prev_token = if matched_prev { None } else { Some(token) };
        remember_resume(&peer_fp, &new_token, &ack, peer_caps, prev_token);
        remember_resume_policy(&peer_fp, &cfg.authentication);
        sink.send(ControlMsg::SessionAccept).await?;
        // Deliver the rotated new token to the client (the resume path has no full negotiation).
        sink.send(ControlMsg::NegotiateReply {
            ack: Box::new(ack.clone()),
        })
        .await?;
        let cancel = CancellationToken::new();
        let (cap_w, cap_h) = fit_capture_dims(main_display.w_px, main_display.h_px);
        let (cmd_tx, cmd_rx, controller, kf_tx, kf_rx, quality_tx, quality_rx) =
            build_session_channels(ack.video.max_bitrate_kbps, ack.video.max_fps, cap_w, cap_h);
        let clip_state = cfg
            .local_clip
            .clone()
            .filter(|_| peer_caps.clipboard)
            .map(|clip| spawn_clip_poller(clip, cmd_tx.clone(), &cancel));
        return Ok((
            EstablishedSession {
                peer_name: hello.hello.device_name.clone(),
                peer_fp_hex: peer_fp,
                ack,
                resume_token: new_token,
                cmd_tx: cmd_tx.clone(),
                controller,
                keyframe_req_tx: kf_tx.clone(),
                quality_tx: quality_tx.clone(),
                peer_caps,
                clip_state,
                cancel,
            },
            kf_rx,
            quality_rx,
            cmd_rx,
            sink,
            source,
        ));
    }

    let mut invitation_authorized = false;
    // First connection: an unknown peer completes pairing inline first (the client opens
    // the pairing stream right after the handshake), and only then do we handle
    // SessionRequest (protocol.md §4.3 → §4.4 ordering).
    if cfg.authentication.mode == removent_core::AuthenticationMode::PairingCode
        && (cfg.authentication.pairing_policy
            == removent_core::authentication::PairingPolicy::EveryConnection
            || hello.feature_bits & removent_proto::feature_bits::PAIRING_INVITATION != 0
            || !peers.by_fingerprint(&peer_fp).is_some_and(|p| p.trusted))
    {
        let peer_name = hello.hello.device_name.clone();
        invitation_authorized = run_pairing(
            &conn,
            identity,
            peers,
            &peer_fp,
            peer_name,
            interactions.show_pairing_pin,
            cfg.auth_paths.as_ref(),
        )
        .await?;
    }

    if matches!(
        cfg.authentication.mode,
        removent_core::AuthenticationMode::Password | removent_core::AuthenticationMode::Otp
    ) {
        if hello.feature_bits & removent_proto::feature_bits::AUTH_METHODS == 0 {
            let _ = sink
                .send(ControlMsg::SessionReject {
                    reason: removent_proto::RejectReason::Denied,
                })
                .await;
            return Err(HostError::Rejected(
                "Update the controller to use this authentication method".into(),
            ));
        }
        if let Err(error) = run_authentication(&conn, identity, &peer_fp, cfg).await {
            let _ = sink
                .send(ControlMsg::SessionReject {
                    reason: removent_proto::RejectReason::Denied,
                })
                .await;
            return Err(error);
        }
    }

    // First connection: wait for SessionRequest.
    let req = expect_msg(&mut source, "SessionRequest", |m| {
        matches!(m, ControlMsg::SessionRequest { .. })
    })
    .await?;
    let ControlMsg::SessionRequest { caps } = req else {
        unreachable!()
    };
    // Unsupported services must neither expand trust nor block unattended
    // video/input behind a prompt for unavailable audio/clipboard/file access.
    let caps = cfg.available_caps(caps);
    // This session always produces a video stream. A request omitting video
    // must not evade the corresponding stored capability grant.
    if !caps.video {
        sink.send(ControlMsg::SessionReject {
            reason: removent_proto::RejectReason::Denied,
        })
        .await?;
        return Err(HostError::Rejected("video capability is required".into()));
    }

    // Admission ruling.
    let admission = if (cfg.authentication.mode != removent_core::AuthenticationMode::PairingCode
        || invitation_authorized)
        && cfg.admission != AdmissionMode::DenyAll
        && !cfg.preapproved_only
    {
        AdmissionDecision::Allow
    } else {
        decide(cfg.admission, peers, &peer_fp, caps)
    };
    match admission {
        AdmissionDecision::Allow => {}
        AdmissionDecision::Ask => {
            if cfg.preapproved_only {
                sink.send(ControlMsg::SessionReject {
                    reason: removent_proto::RejectReason::Denied,
                })
                .await?;
                return Err(HostError::Rejected(
                    "capabilities require prior approval".into(),
                ));
            }
            let short = peer_fp.chars().take(16).collect::<String>();
            // protocol.md §4.4: an admission prompt with no response within 30s is treated as a rejection.
            let prompt = (interactions.admission_prompt)(hello.hello.device_name.clone(), short);
            let allowed = tokio::time::timeout(Duration::from_secs(30), prompt)
                .await
                .unwrap_or(false);
            if !allowed {
                sink.send(ControlMsg::SessionReject {
                    reason: removent_proto::RejectReason::Denied,
                })
                .await?;
                return Err(HostError::Rejected("user denied or prompt timeout".into()));
            }
            // "This time only": peers records the identity only, without a long-term
            // grant (protocol.md §4.3); an existing record (trusted or
            // grant-expansion flow) is not overwritten.
            if peers.by_fingerprint(&peer_fp).is_none() {
                let _ = peers.upsert(PeerRecord {
                    fingerprint: peer_fp.clone(),
                    name: hello.hello.device_name.clone(),
                    short_fp: peer_fp.chars().take(16).collect(),
                    granted_caps: Caps::none(),
                    trusted: false,
                    added_at_unix: now_unix(),
                    last_connected_unix: now_unix(),
                });
            }
        }
        AdmissionDecision::Deny(reason) => {
            sink.send(ControlMsg::SessionReject {
                reason: removent_proto::RejectReason::Denied,
            })
            .await?;
            return Err(HostError::Rejected(reason.into()));
        }
    }

    // Accept and negotiate.
    sink.send(ControlMsg::SessionAccept).await?;
    let display_w = main_display.w_px;
    let display_h = main_display.h_px;
    let displays = vec![main_display];
    let selected = displays[0].id;
    let codec = preferred_video_codec(
        hello.feature_bits,
        display_w,
        display_h,
        cfg.video_bitrate_kbps,
        cfg.video_fps,
    );
    let negotiate = Negotiate {
        displays,
        selected_display: selected,
        video: VideoParams {
            codec,
            max_fps: cfg.video_fps,
            max_bitrate_kbps: cfg.video_bitrate_kbps,
            initial_scale: 1.0,
        },
        audio: removent_proto::AudioParams {
            enabled: caps.audio,
            ..Default::default()
        },
    };
    sink.send(ControlMsg::NegotiateOffer {
        n: Box::new(negotiate.clone()),
    })
    .await?;

    let reply = expect_msg(&mut source, "NegotiateReply", |m| {
        matches!(m, ControlMsg::NegotiateReply { .. })
    })
    .await?;
    let ControlMsg::NegotiateReply { ack } = reply else {
        unreachable!()
    };

    // Issue a resume token and send it back.
    let mut ack = *ack;
    ack.audio.enabled &= caps.audio;
    let token = new_token();
    remember_resume(&peer_fp, &token, &ack, caps, None);
    remember_resume_policy(&peer_fp, &cfg.authentication);
    ack.resume_token = Some(token);
    sink.send(ControlMsg::NegotiateReply {
        ack: Box::new(ack.clone()),
    })
    .await?;

    let cancel = CancellationToken::new();
    let (cap_w, cap_h) = fit_capture_dims(display_w, display_h);
    let (cmd_tx, cmd_rx, controller, kf_tx, kf_rx, quality_tx, quality_rx) =
        build_session_channels(ack.video.max_bitrate_kbps, ack.video.max_fps, cap_w, cap_h);
    let clip_state = cfg
        .local_clip
        .clone()
        .filter(|_| caps.clipboard)
        .map(|clip| spawn_clip_poller(clip, cmd_tx.clone(), &cancel));
    Ok((
        EstablishedSession {
            peer_name: hello.hello.device_name.clone(),
            peer_fp_hex: peer_fp,
            ack,
            resume_token: token,
            cmd_tx,
            controller,
            keyframe_req_tx: kf_tx,
            quality_tx,
            peer_caps: caps,
            clip_state,
            cancel,
        },
        kf_rx,
        quality_rx,
        cmd_rx,
        sink,
        source,
    ))
}

/// AV1 is intentionally opt-in for remote desktop sessions. Software AV1 has
/// materially higher CPU cost and a few frames of encoder pipeline delay, so a
/// peer must advertise the decoder feature and the operator must request it via
/// `REMOVENT_VIDEO_CODEC=av1`. Any failed probe falls back to HEVC.
pub(super) fn preferred_video_codec(
    peer_features: u64,
    display_w: u32,
    display_h: u32,
    bitrate_kbps: u32,
    fps: u8,
) -> CodecId {
    let requested = std::env::var("REMOVENT_VIDEO_CODEC")
        .ok()
        .is_some_and(|v| v.eq_ignore_ascii_case("av1") || v.eq_ignore_ascii_case("software-av1"));
    if !requested || peer_features & removent_proto::feature_bits::SOFTWARE_AV1 == 0 {
        return CodecId::Hevc;
    }
    let (w, h) = fit_capture_dims(display_w, display_h);
    if removent_media_codec::VideoEncoder::new(
        CodecId::Av1,
        w as usize,
        h as usize,
        bitrate_kbps,
        fps,
    )
    .is_ok()
    {
        CodecId::Av1
    } else {
        tracing::warn!("software AV1 probe failed; falling back to HEVC");
        CodecId::Hevc
    }
}

pub(super) async fn run_pairing(
    conn: &RvpConnection,
    identity: &DeviceIdentity,
    peers: &mut PeersStore,
    peer_fp: &str,
    peer_name: String,
    show_pin: Box<dyn FnOnce(String) + Send>,
    paths: Option<&removent_core::DataPaths>,
) -> Result<bool, HostError> {
    // The pairing stream is opened by the admitting side (host): the client accepts
    // while waiting. Bound the wait for the peer to open the stream: without a
    // deadline an unknown peer that stalls after the handshake would hold the
    // single session slot forever (the QUIC keepalive defeats the idle timeout).
    let (mut psink, mut psource) =
        tokio::time::timeout(Duration::from_secs(15), conn.accept_pairing())
            .await
            .map_err(|_| HostError::Timeout("pairing stream"))??;
    // Bound the Begin read the same way (15s stream-arrival budget); the PIN entry
    // window itself stays 300s (protocol.md §4.3).
    let begin = tokio::time::timeout(Duration::from_secs(15), psource.next())
        .await
        .map_err(|_| HostError::Timeout("pairing begin"))?
        .transpose()
        .map_err(HostError::Net)?
        .ok_or(HostError::Timeout("pairing begin"))?;
    // Rate limit per peer BEFORE generating/showing the PIN: an over-limit
    // Begin is rejected without producing a popup.
    if !pairing_begin_limiter()
        .lock()
        .unwrap()
        .allow(peer_fp, std::time::Instant::now())
    {
        tracing::warn!("pairing Begin rate limited, PIN not shown");
        return Err(HostError::Rejected("pairing begin too frequent".into()));
    }
    let invitation = if let removent_net::PairingMsg::BeginInvitation { locator, .. } = &begin {
        // Global bucket prevents attempts using newly generated device identities.
        if !pairing_begin_limiter().lock().unwrap().allow(
            &format!("invite:{}", identity.fingerprint_hex()),
            std::time::Instant::now(),
        ) {
            return Err(HostError::Rejected(
                "Too many invitation attempts; retry in five seconds".into(),
            ));
        }
        let paths =
            paths.ok_or_else(|| HostError::Rejected("Invitation storage unavailable".into()))?;
        Some(
            removent_core::pairing_invitation::Invitation::load(paths)
                .map_err(|e| HostError::Rejected(e.to_string()))?
                .filter(|active| active.code().locator() == locator)
                .ok_or_else(|| HostError::Rejected("Invitation expired or already used".into()))?,
        )
    } else {
        None
    };
    let hs = if let Some(invite) = &invitation {
        removent_net::host_on_begin_with_secret(&begin, invite.code().secret())?
    } else {
        removent_net::host_on_begin(&begin)?
    };
    if invitation.is_none() {
        show_pin(hs.pin.clone());
    }
    psink.send(hs.reply.clone()).await.map_err(HostError::Net)?;

    let verify = tokio::time::timeout(Duration::from_secs(300), psource.next())
        .await
        .map_err(|_| HostError::Timeout("pairing verify"))?
        .transpose()
        .map_err(HostError::Net)?
        .ok_or(HostError::Timeout("pairing verify"))?;
    let (confirm, shared) = removent_net::host_verify(hs, &verify, identity, peer_fp)?;
    let invitation_authorized = invitation.is_some();
    if shared.is_some()
        && let Some(invitation) = invitation
    {
        invitation
            .consume(paths.unwrap())
            .map_err(|e| HostError::Rejected(e.to_string()))?;
    }
    // Deliver the Confirm (including ok=false) to the client before terminating, so the
    // client can attribute the failure to a wrong PIN.
    psink.send(confirm).await.map_err(HostError::Net)?;
    if shared.is_none() {
        return Err(HostError::Rejected("pin mismatch".into()));
    }

    // Reauthentication proves identity without expanding an existing grant.
    // This also preserves capability restrictions in the LoginWindow service.
    let existing = peers.by_fingerprint(peer_fp).filter(|p| p.trusted);
    let granted_caps = existing.map(|p| p.granted_caps).unwrap_or_else(Caps::all);
    let added_at_unix = existing.map(|p| p.added_at_unix).unwrap_or_else(now_unix);
    peers
        .upsert(PeerRecord {
            fingerprint: peer_fp.to_string(),
            name: peer_name,
            short_fp: peer_fp.chars().take(16).collect(),
            granted_caps,
            trusted: true,
            added_at_unix,
            last_connected_unix: now_unix(),
        })
        .map_err(|e| HostError::Rejected(e.to_string()))?;
    Ok(invitation_authorized)
}

pub(super) fn new_token() -> [u8; 16] {
    let mut t = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut t);
    t
}

// ---------------- media loops ----------------

/// Password and TOTP use the same PAKE identity/transcript binding as pairing.
/// TOTP offers three independently blinded candidates for +/- one 30s clock step.
async fn run_authentication(
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
    let candidates = if cfg.authentication.mode == AuthenticationMode::Password {
        vec![(0, cfg.authentication.password.clone())]
    } else {
        let totp = cfg.authentication.totp().map_err(HostError::Rejected)?;
        [step.saturating_sub(1), step, step + 1]
            .into_iter()
            .map(|step| (step, totp.generate(step * 30)))
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
