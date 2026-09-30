use super::*;

pub(super) fn endpoint(
    address: SocketAddr,
    identity: &DeviceIdentity,
    pin: Option<[u8; 32]>,
) -> Result<removent_net::quinn::Endpoint> {
    let bind = if address.is_ipv6() {
        "[::]:0"
    } else {
        "0.0.0.0:0"
    }
    .parse()?;
    Ok(removent_net::make_client_endpoint(
        bind,
        identity,
        removent_net::PinState::new(pin, pin.is_none()),
    )?
    .0)
}

pub(super) async fn run_native(
    request: ConnectionRequest,
    audio: bool,
    clipboard: bool,
    attempt: Attempt,
) -> Result<()> {
    let identity = &attempt.shared.identity;
    let mut bridge = None;
    let mut expected = request.relay.as_ref().map(|r| r.host_pin());
    let address = if let Some(route) = &request.relay {
        attempt.progress("Connecting");
        let config = removent_relay::config::TunnelConfig {
            server: route.endpoint.clone(),
            transport: route.transport,
            insecure_loopback: false,
            server_fingerprint: route.server_fingerprint.clone(),
            room: request.address.host.clone(),
            token: request.password.clone(),
            host_fingerprint: route.host_fingerprint.clone(),
        };
        bridge = Some(removent_relay::client::ClientBridge::start(&config, identity).await?);
        bridge.as_ref().unwrap().address
    } else {
        addresses(&request).await?[0]
    };
    let pin_path = attempt.shared.paths.root.join("mobile-host-pins.json");
    let mut pins: std::collections::BTreeMap<String, String> = match std::fs::read(&pin_path) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Default::default(),
        Err(e) => return Err(e.into()),
    };
    let pin_key = request.address.to_string();
    if request.relay.is_none()
        && let Some(pin) = pins.get(&pin_key)
    {
        expected = Some(
            hex::decode(pin)?
                .try_into()
                .map_err(|_| anyhow::anyhow!("Invalid saved host fingerprint"))?,
        );
    }
    let clip = MemoryClipboard::new();
    let config = || removent_client::ClientConfig {
        device_name: identity.device_name.clone(),
        caps: Caps {
            video: true,
            input: true,
            audio,
            clipboard,
            file: false,
        },
        local_clip: if clipboard { Some(clip.clone()) } else { None },
    };
    let pin_attempt = attempt.clone();
    attempt.progress("Negotiating");
    let mut session = removent_client::connect_session(
        endpoint(address, identity, expected)?,
        address,
        identity,
        config(),
        None,
        None,
        Some(Box::new(move |pin| {
            pin_attempt.update(|s| {
                s.pin = Some(pin);
                s.event(json!({"type":"pin", "generation":pin_attempt.generation}));
            });
        })),
    )
    .await?;
    let peer_pin = session
        .conn
        .peer_fingerprint()
        .context("Host certificate missing")?;
    expected = Some(peer_pin); // Resumes must authenticate the exact initial host.
    if request.relay.is_none() {
        pins.insert(pin_key, hex::encode(peer_pin));
        removent_core::settings::atomic_write(&pin_path, &serde_json::to_vec(&pins)?)?;
    }
    let mut resumed_once = false;
    loop {
        let params = session.negotiated.audio;
        attempt.update(|s| {
            s.input = Some(InputChannel::Ordered(session.input_tx.clone()));
            s.control = Some(session.cmd_tx.clone());
            s.pin = None;
            s.clipboard = clipboard.then(|| clip.clone());
            s.audio_rate = params.sample_rate;
            s.audio_channels = params.channels;
        });
        attempt.event(
            json!({"type":"ready", "codec":format!("{:?}", session.negotiated.video.codec),
            "audio":params.enabled, "sample_rate":params.sample_rate, "channels":params.channels,
            "clipboard":clipboard, "refresh":true, "fingerprint":hex::encode(peer_pin)}),
        );
        let mut pcm_open = params.enabled;
        loop {
            tokio::select! {
                frame = session.decoded_bgra_rx.recv() => {
                    match frame { Some(f) => attempt.frame(f), None => break }
                }
                pcm = session.decoded_pcm_rx.recv(), if pcm_open => {
                    match pcm {
                        Some(pcm) => attempt.update(|s| {
                            if s.audio.len() >= 12 { s.audio.pop_front(); }
                            s.audio.push_back(pcm);
                        }),
                        None => pcm_open = false,
                    }
                }
            }
        }
        attempt.update(|s| {
            s.input = None;
            s.control = None;
            s.audio.clear();
        });
        if !session.was_interrupted() {
            return Ok(());
        }
        ensure!(
            !resumed_once,
            "Connection interrupted; reconnect to the host"
        );
        let token = session
            .current_resume_token()
            .context("Connection interrupted")?;
        let ack = session.negotiated.clone();
        drop(session);
        attempt.progress("Reconnecting");
        // Retain the relay bridge for the complete session, including quick resume.
        let _keep_bridge = &bridge;
        let mut next = None;
        for retry in 0..3 {
            if retry > 0 {
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            if let Ok(Ok(s)) = tokio::time::timeout(
                Duration::from_secs(8),
                removent_client::quick_resume(
                    endpoint(address, identity, expected)?,
                    address,
                    identity,
                    config(),
                    token,
                    ack.clone(),
                ),
            )
            .await
            {
                next = Some(s);
                break;
            }
        }
        session = next.context("Connection interrupted; automatic reconnect failed")?;
        resumed_once = true;
    }
}
