use super::*;

fn connection_failure(error: removent_client::ConnectError, addr: SocketAddr) -> anyhow::Error {
    use removent_client::ConnectError;
    let unreachable = match &error {
        ConnectError::Rejected(message) => message == "quinn: timed out",
        ConnectError::Net(removent_net::NetError::Tls(message)) => {
            message == "timed out" || message == "Certificate connection timed out"
        }
        _ => false,
    };
    if unreachable {
        anyhow::anyhow!(
            t!(
                "connection.native_unreachable",
                address = addr.to_string(),
                port = addr.port()
            )
            .to_string()
        )
    } else {
        error.into()
    }
}

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
    pub video_quality: removent_proto::VideoQuality,
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
        video_quality,
        address: addr,
        expected_host,
        key: destination_key,
        invitation,
    } = destination;
    let pairing_code = invitation.as_ref().and_then(|r| r.pairing_code.clone());
    let direct = destination_key.is_none();
    let pin_key = destination_key.unwrap_or_else(|| addr.to_string());
    let trust = removent_client::host_pins::for_connection(
        &paths,
        &pin_key,
        expected_host,
        pairing_code.is_some(),
    )?;
    let expected_host = trust.expected;
    if let Some(pin) = expected_host {
        attempt.claim_fingerprint(pin)?;
    }
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
        video_quality,
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
    let pending_auth = Arc::new(Mutex::new(None));
    let local_identity = identity.fingerprint_hex();
    let pin_request = credentials::prompt(
        attempt.clone(),
        local_identity.clone(),
        // Invitations may replace the host identity; never prefill its old credential.
        expected_host.filter(|_| pairing_code.is_none()),
        pending_auth.clone(),
    );
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
            Some(pin_request),
            confirmation,
        )
        .await
        .map_err(|error| {
            if direct {
                connection_failure(error, addr)
            } else {
                error.into()
            }
        })?
    };

    let peer_pin = session
        .conn
        .peer_fingerprint()
        .ok_or_else(|| anyhow::anyhow!("Host certificate missing"))?;
    attempt.claim_fingerprint(peer_pin)?;
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
    let submitted_auth = pending_auth.lock().unwrap().take();
    if let Some(submitted) = submitted_auth {
        let events = attempt.events.clone();
        // A locked Keychain must not delay frame/audio forwarding.
        tokio::spawn(async move {
            if let Err(error) = submitted.commit(local_identity, peer_pin).await {
                tracing::warn!(%error, "could not update native host credentials in Keychain");
                let _ = events.send(UiEvent::Notice(t!("auth.keychain_save_failed").to_string()));
            }
        });
    }
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

#[cfg(test)]
mod failure_tests {
    use super::*;

    #[test]
    fn unavailable_service_has_a_recovery_action_but_authentication_errors_are_preserved() {
        let addr = "127.0.0.1:48688".parse().unwrap();
        for error in [
            removent_client::ConnectError::Rejected("quinn: timed out".into()),
            removent_client::ConnectError::Net(removent_net::NetError::Tls("timed out".into())),
            removent_client::ConnectError::Net(removent_net::NetError::Tls(
                "Certificate connection timed out".into(),
            )),
        ] {
            let message = connection_failure(error, addr).to_string();
            assert!(message.contains("48688"));
            assert!(message.contains("UDP"));
        }
        let error = removent_client::ConnectError::Pairing("incorrect password".into());
        assert_eq!(
            connection_failure(error, addr).to_string(),
            "pairing failed: incorrect password"
        );
    }
}
