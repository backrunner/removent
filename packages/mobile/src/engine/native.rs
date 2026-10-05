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
    let mut expected = request.relay.as_ref().and_then(|r| r.optional_host_pin());
    let address = if let Some(route) = &request.relay {
        attempt.progress("Connecting");
        let mut config = removent_relay::config::TunnelConfig {
            server: route.endpoint.clone(),
            transport: route.transport,
            insecure_loopback: false,
            server_fingerprint: route.server_fingerprint.clone(),
            server_name: route.server_name.clone(),
            accept_invalid_certificate: route.accept_invalid_certificate,
            room: request.address.host.clone(),
            token: request.password.clone(),
            host_fingerprint: route.host_fingerprint.clone(),
        };
        let relay_key = format!(
            "relay-server:{}:{}",
            config.server,
            config.tls_server_name()?
        );
        let relay_trust = removent_client::host_pins::for_relay_connection(
            &attempt.shared.paths,
            &relay_key,
            if config.server_fingerprint.is_empty() {
                None
            } else {
                Some(removent_relay::config::decode_secret(
                    &config.server_fingerprint,
                )?)
            },
            !config.is_websocket() && !config.accept_invalid_certificate,
        )?;
        if !config.is_websocket() {
            config.server_fingerprint = relay_trust.expected.map(hex::encode).unwrap_or_default();
        }
        let confirmation = (!config.is_websocket()
            && relay_trust.needs_confirmation
            && !config.accept_invalid_certificate)
            .then(|| attempt.certificate_confirmation(config.server.clone(), true));
        bridge = Some(
            removent_relay::client::ClientBridge::start_with_confirmation(
                &config,
                identity,
                confirmation,
            )
            .await?,
        );
        if !config.accept_invalid_certificate
            && let Some(pin) = bridge.as_ref().unwrap().relay_fingerprint
        {
            removent_client::host_pins::remember(&attempt.shared.paths, &relay_key, pin, false)?;
        }
        bridge.as_ref().unwrap().address
    } else {
        if let Some(code) = &request.pairing_code {
            #[cfg(target_os = "ios")]
            {
                let (tx, rx) = oneshot::channel();
                attempt.update(|s| { s.pairing_address = Some(tx); s.event(json!({"type":"resolve_pairing", "locator":code.locator(), "generation":attempt.generation})); });
                let address = tokio::time::timeout(Duration::from_secs(15), rx)
                    .await
                    .context("Pairing lookup timed out")?
                    .context("Pairing lookup cancelled")?
                    .map_err(anyhow::Error::msg)?;
                match address.to_string().parse::<SocketAddr>() {
                    Ok(address) => address,
                    Err(_) => tokio::time::timeout(
                        Duration::from_secs(5),
                        tokio::net::lookup_host((address.host.as_str(), address.port)),
                    )
                    .await
                    .context("Pairing destination lookup timed out")??
                    .next()
                    .context("Pairing destination unavailable")?,
                }
            }
            #[cfg(not(target_os = "ios"))]
            {
                removent_net::discovery::resolve_pairing_locator(code.locator()).await?
            }
        } else {
            addresses(&request).await?[0]
        }
    };
    let pin_key = if let Some(route) = &request.relay {
        format!(
            "relay:{}:{}",
            route.endpoint,
            bridge
                .as_ref()
                .and_then(|b| b.resolved_room.as_deref())
                .unwrap_or(&request.address.host)
        )
    } else if request.pairing_code.is_some() {
        address.to_string()
    } else {
        request.address.to_string()
    };
    let trust = removent_client::host_pins::for_connection(
        &attempt.shared.paths,
        &pin_key,
        expected,
        request.pairing_code.is_some(),
    )?;
    expected = trust.expected;
    let clip = MemoryClipboard::new();
    let config = || removent_client::ClientConfig {
        pairing_code: request.pairing_code.clone(),
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
    let destination = if let Some(route) = &request.relay {
        format!(
            "{} · {}",
            bridge
                .as_ref()
                .and_then(|b| b.resolved_room.as_deref())
                .unwrap_or(&request.address.host),
            route.endpoint
        )
    } else if request.pairing_code.is_some() {
        address.to_string()
    } else {
        request.address.to_string()
    };
    let confirmation = trust
        .needs_confirmation
        .then(|| attempt.certificate_confirmation(destination, false));
    let mut session = removent_client::connect_session_with_confirmation(
        endpoint(address, identity, expected)?,
        address,
        identity,
        config(),
        None,
        None,
        Some(Box::new(move |mode, pin| {
            pin_attempt.update(|s| {
                s.pin = Some(pin);
                s.auth_mode = mode;
                s.event(json!({"type":"pin", "mode":mode, "generation":pin_attempt.generation}));
            });
        })),
        confirmation,
    )
    .await?;
    let peer_pin = session
        .conn
        .peer_fingerprint()
        .context("Host certificate missing")?;
    expected = Some(peer_pin); // Resumes must authenticate the exact initial host.
    {
        removent_client::host_pins::remember(
            &attempt.shared.paths,
            &pin_key,
            peer_pin,
            request.pairing_code.is_some(),
        )?;
    }
    let resolved = request.pairing_code.as_ref().map(|_| {
        let mut route = request.relay.clone();
        if let Some(route) = &mut route {
            route.host_fingerprint = hex::encode(peer_pin);
        }
        let host = bridge
            .as_ref()
            .and_then(|b| b.resolved_room.clone())
            .unwrap_or_else(|| {
                removent_client::connection::ConnectionAddress::from_socket(address).host
            });
        json!({"host":host, "port":if route.is_some() { 0 } else { address.port() }, "relay":route})
    });
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
            "clipboard":clipboard, "refresh":true, "fingerprint":hex::encode(peer_pin), "resolved":resolved}),
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
                    {
                        let mut cfg = config();
                        cfg.pairing_code = None;
                        cfg
                    },
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
