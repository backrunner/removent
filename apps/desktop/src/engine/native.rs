use super::*;

pub(super) struct AudioForwardTask(tokio::task::JoinHandle<()>);
impl Drop for AudioForwardTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(super) fn client_bind_addr(peer: SocketAddr) -> SocketAddr {
    if peer.is_ipv6() {
        SocketAddr::from(([0u16; 8], 0))
    } else {
        SocketAddr::from(([0u8; 4], 0))
    }
}

pub(super) async fn run_client(
    identity: DeviceIdentity,
    addr: SocketAddr,
    paths: DataPaths,
    settings: Settings,
    attempt: ClientAttempt,
    expected_host: Option<[u8; 32]>,
) -> Result<()> {
    let known = known_fingerprints(&paths);
    let (ep_client, _pin) = make_client_endpoint(
        client_bind_addr(addr),
        &identity,
        match expected_host {
            Some(pin) => PinState::new([pin], false),
            None => PinState::new(known, true),
        },
    )?;

    let audio_player = match tokio::task::spawn_blocking(AudioPlayer::new).await? {
        Ok(player) => Some(player),
        Err(e) => {
            tracing::warn!(err = %e, "audio output unavailable; negotiating video only");
            None
        }
    };
    let audio_enabled = audio_player.is_some();

    let mk_cfg = || removent_client::ClientConfig {
        device_name: settings.device_name.clone(),
        caps: Caps {
            audio: audio_enabled,
            ..Caps::all()
        },
        local_clip: Some(Arc::new(NsClipboard)),
    };

    // The PIN prompt is deferred until pairing actually starts: a trusted peer
    // never needs one (no popup flash on every connect).
    let pin_attempt = attempt.clone();
    attempt.progress(ConnectionStage::Negotiating);
    let mut session = connect_session(
        ep_client,
        addr,
        &identity,
        mk_cfg(),
        None,
        None,
        Some(Box::new(move |pin_tx| {
            pin_attempt.progress(ConnectionStage::Pairing);
            let _ = pin_attempt.events.send(UiEvent::ClientNeedsPin {
                generation: pin_attempt.generation,
                tx: pin_tx,
            });
        })),
    )
    .await?;

    // Frame bridge: install the channel before telling the UI to open the viewer
    // (eliminates the race of not being able to take rx).
    let ftx = attempt.publish(
        session.input_tx.clone(),
        format!("{:?}", session.negotiated.video.codec),
    )?;
    let mut audio_task = audio_player.as_ref().map(|player| {
        let player = player.clone();
        let (_drop_tx, drop_rx) = tokio::sync::mpsc::channel(1);
        let pcm_rx = std::mem::replace(&mut session.decoded_pcm_rx, drop_rx);
        AudioForwardTask(tokio::spawn(async move {
            let mut pcm_rx = pcm_rx;
            while let Some(pcm) = pcm_rx.recv().await {
                player.push(pcm);
            }
        }))
    });
    loop {
        while let Some(frame) = session.decoded_bgra_rx.recv().await {
            if ftx.send(frame).is_err() {
                return Ok(());
            }
        }
        // The frame channel closed: a clean SessionEnd ends here; an abnormal
        // network drop gets one transparent quick-resume attempt (§7.3/§7.4) —
        // on success the session continues without the UI noticing.
        attempt.pause_input();
        if let Some(mut task) = audio_task.take() {
            task.0.abort();
            let _ = (&mut task.0).await;
        }
        if let Some(player) = &audio_player {
            player.clear();
        }
        if !session.was_interrupted() {
            return Ok(());
        }
        let Some(token) = session.current_resume_token() else {
            return Ok(());
        };
        let prev_ack = session.negotiated.clone();
        // A media stream can fail while QUIC/control remain live. Release that
        // session before retrying, or it keeps the host busy across all retries.
        drop(session);
        tracing::info!("connection dropped abnormally; attempting quick resume");
        // The old session's permit on the host is released at the end of its
        // teardown chain, so a resume attempted immediately after a drop can
        // be rejected with Busy. Retry with a short backoff (well inside the
        // 30s resume window) before reporting the session as closed.
        let mut resumed = None;
        for attempt in 0..3 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            }
            let known = known_fingerprints(&paths);
            let (ep, _pin) = make_client_endpoint(
                client_bind_addr(addr),
                &identity,
                match expected_host {
                    Some(pin) => PinState::new([pin], false),
                    None => PinState::new(known, true),
                },
            )?;
            match tokio::time::timeout(
                std::time::Duration::from_secs(8),
                removent_client::quick_resume(
                    ep,
                    addr,
                    &identity,
                    mk_cfg(),
                    token,
                    prev_ack.clone(),
                ),
            )
            .await
            {
                Ok(Ok(s)) => {
                    resumed = Some(s);
                    break;
                }
                Ok(Err(e)) => {
                    tracing::warn!(err=%e, attempt, "quick resume attempt failed");
                }
                Err(_) => tracing::warn!(attempt, "quick resume attempt timed out"),
            }
        }
        match resumed {
            Some(s) => {
                session = s;
                attempt.resume(session.input_tx.clone())?;
                audio_task = audio_player.as_ref().map(|player| {
                    let player = player.clone();
                    let (_drop_tx, drop_rx) = tokio::sync::mpsc::channel(1);
                    let pcm_rx = std::mem::replace(&mut session.decoded_pcm_rx, drop_rx);
                    AudioForwardTask(tokio::spawn(async move {
                        let mut pcm_rx = pcm_rx;
                        while let Some(pcm) = pcm_rx.recv().await {
                            player.push(pcm);
                        }
                    }))
                });
            }
            None => return Ok(()),
        }
    }
}
