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

pub(super) struct NativeDestination {
    pub address: SocketAddr,
    pub expected_host: Option<[u8; 32]>,
    pub key: Option<String>,
    pub invitation: Option<ConnectionRequest>,
}

pub(super) async fn run_client(
    identity: DeviceIdentity,
    destination: NativeDestination,
    paths: DataPaths,
    settings: Settings,
    attempt: ClientAttempt,
) -> Result<()> {
    let NativeDestination {
        address: addr,
        expected_host,
        key: destination_key,
        invitation,
    } = destination;
    let pairing_code = invitation.as_ref().and_then(|r| r.pairing_code.clone());
    let pin_key = destination_key.unwrap_or_else(|| addr.to_string());
    let trust = removent_client::host_pins::for_connection(
        &paths,
        &pin_key,
        expected_host,
        pairing_code.is_some(),
    )?;
    let expected_host = trust.expected;
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
        host_identity_verified: expected_host.is_some() || trust.needs_confirmation,
        pairing_code: pairing_code.clone(),
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
    let destination = pin_key
        .strip_prefix("relay:")
        .and_then(|route| route.rsplit_once(':'))
        .map(|(endpoint, room)| format!("{room} ({endpoint})"))
        .unwrap_or_else(|| addr.to_string());
    let confirmation = trust
        .needs_confirmation
        .then(|| attempt.certificate_confirmation(destination, false));
    let resume = attempt.recovery.native.lock().unwrap().clone();
    let mut session = if let Some(resume) = resume {
        resume.reconnect(addr, &identity, mk_cfg()).await?
    } else {
        removent_client::connect_session_with_confirmation(
            ep_client,
            addr,
            &identity,
            mk_cfg(),
            None,
            None,
            Some(Box::new(move |mode, pin_tx| {
                pin_attempt.progress(if mode == removent_core::AuthenticationMode::PairingCode {
                    ConnectionStage::Pairing
                } else {
                    ConnectionStage::Authenticating
                });
                let _ = pin_attempt.events.send(UiEvent::ClientNeedsPin {
                    generation: pin_attempt.generation,
                    mode,
                    tx: pin_tx,
                });
            })),
            confirmation,
        )
        .await?
    };

    let peer_pin = session
        .conn
        .peer_fingerprint()
        .ok_or_else(|| anyhow::anyhow!("Host certificate missing"))?;
    *attempt.recovery.native.lock().unwrap() = Some(removent_client::recovery::NativeResume {
        token: session.resume_token.clone(),
        negotiated: session.negotiated.clone(),
        fingerprint: peer_pin,
    });
    removent_client::host_pins::remember(&paths, &pin_key, peer_pin, pairing_code.is_some())?;
    if let Some(mut request) = invitation {
        request.pairing_code = None;
        if let Some(route) = &mut request.relay {
            route.host_fingerprint = hex::encode(peer_pin);
        }
        *attempt.recovery.destination.lock().unwrap() = Some(request.clone());
        let result = SavedConnections::load(&paths).and_then(|mut s| {
            s.save_with_password(
                SavedConnection::from_request(&request, ""),
                &request.password,
            )
        });
        match result {
            Ok(_) => {
                if let Ok(saved) = SavedConnections::load(&paths) {
                    let _ = attempt.events.send(UiEvent::PairedConnections {
                        generation: attempt.generation,
                        entries: saved.all().to_vec(),
                    });
                }
            }
            Err(e) => {
                let _ = attempt.events.send(UiEvent::Notice(
                    t!("status.connection_save_failed", err = e.to_string()).to_string(),
                ));
            }
        }
    }
    // Frame bridge: install the channel before telling the UI to open the viewer
    // (eliminates the race of not being able to take rx).
    let ftx = attempt.publish(
        session.input_tx.clone(),
        format!("{:?}", session.negotiated.video.codec),
    )?;
    let _audio_task = audio_player.as_ref().map(|player| {
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
    while let Some(frame) = session.decoded_bgra_rx.recv().await {
        if ftx.send(frame).is_err() {
            return Ok(());
        }
    }
    attempt.pause_input();
    if let Some(player) = &audio_player {
        player.clear();
    }
    anyhow::ensure!(!session.was_interrupted(), "Connection interrupted");
    Ok(())
}
