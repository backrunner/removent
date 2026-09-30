use super::*;

pub(super) fn vnc_failure_message(error: &removent_client::VncError) -> String {
    use removent_client::VncError;
    match error {
        VncError::Timeout { stage, seconds } => {
            let key = match *stage {
                "TCP connection" => "connection.vnc_tcp",
                "server greeting" => "connection.vnc_greeting",
                "security negotiation" => "connection.vnc_negotiation",
                "authentication challenge" | "authentication result" => {
                    "connection.vnc_authentication"
                }
                "desktop initialization" => "connection.vnc_desktop",
                _ => return error.to_string(),
            };
            t!("connection.vnc_timeout", stage = t!(key), seconds = seconds).to_string()
        }
        VncError::Authentication => t!("connection.vnc_auth_failed").to_string(),
        VncError::Io(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
            t!("connection.vnc_refused").to_string()
        }
        _ => error.to_string(),
    }
}

/// Connect to a standard RFB/VNC server and bridge its raw frames into the
/// same viewer bus used by RVP. VNC has no Removent pairing/resume channel, so
/// a disconnect is reported directly to the UI.
pub(super) async fn run_vnc_client(
    request: ConnectionRequest,
    attempt: ClientAttempt,
) -> Result<()> {
    let target = request.address.to_string();
    let progress = attempt.progress_sink();
    attempt.progress(ConnectionStage::Resolving);
    let mut session = tokio::time::timeout(std::time::Duration::from_secs(90), async {
        let addresses = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio::net::lookup_host((request.address.host.as_str(), request.address.port)),
        )
        .await
        .map_err(|_| anyhow::anyhow!("VNC host lookup timed out for {target}"))??;
        let mut last_error = anyhow::anyhow!("No addresses found for {target}");
        for addr in addresses {
            match removent_client::vnc::connect_vnc_with_progress(
                addr,
                &request.username,
                &request.password,
                Some(&progress),
            )
            .await
            {
                Ok(session) => return Ok(session),
                Err(e) => {
                    tracing::warn!(%addr, err = %e, "VNC connection failed");
                    // Retrying another DNS address cannot fix account credentials.
                    let authentication_failed =
                        matches!(e, removent_client::VncError::Authentication);
                    last_error = anyhow::anyhow!("{addr}: {}", vnc_failure_message(&e));
                    if authentication_failed {
                        break;
                    }
                }
            }
        }
        Err(last_error)
    })
    .await
    .map_err(|_| {
        anyhow::anyhow!("VNC connection to {target} exceeded 90 seconds during server negotiation")
    })??;
    let ftx = attempt.publish(
        ClientCommands::Vnc(session.cmd_tx.clone(), session.stats.clone()),
        "RFB/VNC · BGRA".into(),
    )?;
    while let Some(frame) = session.decoded_bgra_rx.recv().await {
        if ftx.send(frame).is_err() {
            return Ok(());
        }
    }
    (&mut session.completion)
        .await
        .map_err(|_| anyhow::anyhow!("VNC session task stopped unexpectedly"))??;
    Ok(())
}

pub(super) async fn run_requested_client(
    identity: Option<DeviceIdentity>,
    request: ConnectionRequest,
    paths: DataPaths,
    settings: Settings,
    attempt: ClientAttempt,
) -> Result<()> {
    match request.protocol {
        ConnectionProtocol::Removent => {
            attempt.progress(ConnectionStage::Resolving);
            if let Some(route) = &request.relay {
                let route = removent_client::connection::RelayRoute::parse(
                    &route.endpoint,
                    route.transport,
                    &route.server_fingerprint,
                    &route.host_fingerprint,
                )
                .map_err(|key| anyhow::anyhow!(t!(key).to_string()))?;
                let room = &request.address.host;
                let config = removent_relay::config::TunnelConfig {
                    server: route.endpoint.clone(),
                    transport: route.transport,
                    insecure_loopback: false,
                    server_fingerprint: route.server_fingerprint.clone(),
                    room: room.to_owned(),
                    token: request.password.clone(),
                    host_fingerprint: route.host_fingerprint.clone(),
                };
                let identity =
                    identity.ok_or_else(|| anyhow::anyhow!("Device identity missing"))?;
                attempt.progress(ConnectionStage::Connecting);
                let bridge =
                    removent_relay::client::ClientBridge::start(&config, &identity).await?;
                return run_client(
                    identity,
                    bridge.address,
                    paths,
                    settings,
                    attempt,
                    Some(route.host_pin()),
                )
                .await;
            }
            let addr = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                tokio::net::lookup_host((request.address.host.as_str(), request.address.port)),
            )
            .await??
            .next()
            .ok_or_else(|| anyhow::anyhow!("No addresses found for {}", request.address.host))?;
            run_client(
                identity.ok_or_else(|| anyhow::anyhow!("Device identity missing"))?,
                addr,
                paths,
                settings,
                attempt,
                None,
            )
            .await
        }
        ConnectionProtocol::Vnc => run_vnc_client(request, attempt).await,
        ConnectionProtocol::Rdp => {
            let mut session = removent_client::rdp::connect_rdp_with_progress(
                request,
                Some(attempt.progress_sink()),
            )
            .await?;
            let ftx = attempt.publish(session.cmd_tx.clone(), "RDP".into())?;
            while let Some(frame) = session.decoded_bgra_rx.recv().await {
                if ftx.send(frame).is_err() {
                    return Ok(());
                }
            }
            (&mut session.completion)
                .await
                .map_err(|_| anyhow::anyhow!("RDP session task stopped unexpectedly"))?
        }
    }
}
