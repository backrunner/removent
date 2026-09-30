use super::*;

pub(super) async fn addresses(request: &ConnectionRequest) -> Result<Vec<SocketAddr>> {
    let addresses: Vec<_> = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::net::lookup_host((request.address.host.as_str(), request.address.port)),
    )
    .await
    .context("Host lookup timed out")??
    .collect();
    ensure!(!addresses.is_empty(), "Host has no addresses");
    Ok(addresses)
}

pub(super) async fn run(
    request: ConnectionRequest,
    audio: bool,
    clipboard: bool,
    attempt: Attempt,
) -> Result<()> {
    attempt.progress("Resolving");
    match request.protocol {
        ConnectionProtocol::Removent => run_native(request, audio, clipboard, attempt).await,
        ConnectionProtocol::Vnc => {
            let mut session = tokio::time::timeout(Duration::from_secs(90), async {
                let mut error = anyhow::anyhow!("Host has no addresses");
                for address in addresses(&request).await? {
                    match removent_client::vnc::connect_vnc_with_progress(
                        address,
                        &request.username,
                        &request.password,
                        Some(&attempt.progress_sink()),
                    )
                    .await
                    {
                        Ok(s) => return Ok(s),
                        Err(e) => {
                            let authentication =
                                matches!(e, removent_client::VncError::Authentication);
                            error = e.into();
                            if authentication {
                                break;
                            }
                        }
                    }
                }
                Err(error)
            })
            .await
            .context("VNC connection timed out")??;
            attempt.update(|s| s.input = Some(InputChannel::Ordered(session.cmd_tx.clone())));
            attempt.event(json!({"type":"ready", "codec":"VNC", "audio":false, "clipboard":false}));
            while let Some(frame) = session.decoded_bgra_rx.recv().await {
                attempt.frame(frame);
            }
            (&mut session.completion)
                .await
                .context("VNC session stopped")??;
            Ok(())
        }
        ConnectionProtocol::Rdp => {
            let mut session = removent_client::rdp::connect_rdp_with_progress(
                request,
                Some(attempt.progress_sink()),
            )
            .await?;
            attempt.update(|s| s.input = Some(InputChannel::Rdp(session.cmd_tx.clone())));
            attempt.event(json!({"type":"ready", "codec":"RDP", "audio":false, "clipboard":false}));
            while let Some(frame) = session.decoded_bgra_rx.recv().await {
                attempt.frame(frame);
            }
            (&mut session.completion)
                .await
                .context("RDP session stopped")?
        }
    }
}
