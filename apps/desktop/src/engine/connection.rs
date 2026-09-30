use super::*;

impl Engine {
    /// Connect to the peer at the given address and enter a viewing session.
    pub fn connect_to(&self, addr: SocketAddr) -> Result<()> {
        self.connect_request(ConnectionRequest::native(addr))
    }

    pub fn connect_request(&self, request: ConnectionRequest) -> Result<()> {
        // Serialize the check, generation change and task installation with disconnect.
        let mut task = self.client_task.lock().unwrap();
        if task.as_ref().is_some_and(|t| !t.is_finished()) {
            anyhow::bail!(t!("err.session_in_progress").to_string());
        }
        let identity = if request.protocol == ConnectionProtocol::Removent {
            Some(self.identity()?)
        } else {
            None
        };
        let paths = self.paths.clone();
        let settings = self.settings.lock().unwrap().clone();
        let attempt = ClientAttempt {
            generation: self.client_channels.lock().unwrap().invalidate(),
            started: std::time::Instant::now(),
            channels: self.client_channels.clone(),
            events: self.events_tx.clone(),
        };
        // Record only the destination and protocol, never the request/credentials.
        let span = tracing::info_span!("client_connection", pid = std::process::id(), attempt = attempt.generation,
            protocol = ?request.protocol, destination = %request.address);
        tracing::info!(parent: &span, "connection requested");
        let handle = self.rt.spawn(
            async move {
                // catch_unwind: even a task panic must reset the UI (the connect button
                // must not get stuck on "connecting…").
                let result = std::panic::AssertUnwindSafe(run_requested_client(
                    identity,
                    request,
                    paths,
                    settings,
                    attempt.clone(),
                ))
                .catch_unwind()
                .await;
                attempt.finish(match result {
                    Ok(Ok(())) => None,
                    Ok(Err(e)) => Some(format!("{e:#}")),
                    Err(_) => Some(t!("session.internal_error").to_string()),
                });
            }
            .instrument(span),
        );
        *task = Some(handle);
        Ok(())
    }

    pub fn client_generation(&self) -> usize {
        self.client_channels.lock().unwrap().generation
    }

    pub fn take_client_frames(&self) -> Option<removent_core::latest::Receiver<VideoFrame>> {
        self.client_channels.lock().unwrap().frames.take()
    }

    /// Closing an old disconnected viewer must not terminate a newer session.
    pub fn disconnect_client_if(&self, generation: usize) {
        self.cancel_client(
            Some(generation),
            t!("session.manually_disconnected").to_string(),
        );
    }

    /// Disconnect the session proactively; only sends SessionClosed when a live task
    /// exists (avoids a bogus event when closing the window).
    pub fn disconnect_client(&self) {
        self.cancel_client(None, t!("session.manually_disconnected").to_string());
    }

    pub(super) fn cancel_client(&self, expected_generation: Option<usize>, reason: String) {
        let mut task = self.client_task.lock().unwrap();
        let mut channels = self.client_channels.lock().unwrap();
        if expected_generation.is_some_and(|generation| generation != channels.generation) {
            return;
        }
        // Also invalidate already-queued ready/PIN/error events from a finished task.
        let previous = channels.generation;
        let generation = channels.invalidate();
        if let Some(task) = task.take() {
            if !task.is_finished() {
                tracing::info!(
                    attempt = previous,
                    "connection or session cancelled locally"
                );
            }
            task.abort();
            let _ = self
                .events_tx
                .send(UiEvent::SessionClosed { generation, reason });
        }
    }

    // ---- viewer input / clipboard pass-through ----

    pub fn client_diagnostics(&self, generation: usize) -> ClientDiagnostics {
        let channels = self.client_channels.lock().unwrap();
        if channels.generation != generation {
            return ClientDiagnostics::default();
        }
        let mut diagnostics = ClientDiagnostics {
            codec: channels.codec.clone(),
            ..Default::default()
        };
        if let Some(ClientCommands::Vnc(input, stats)) = &channels.cmd {
            diagnostics.input = Some(input.snapshot());
            diagnostics.vnc = Some(stats.snapshot());
        }
        diagnostics
    }
}
