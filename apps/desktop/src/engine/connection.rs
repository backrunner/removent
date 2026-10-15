use super::*;

impl Engine {
    /// Connect to the peer at the given address and enter a viewing session.
    pub fn connect_to(&self, addr: SocketAddr) -> Result<Self> {
        self.connect_request(ConnectionRequest::native(addr))
    }

    /// Returns a handle scoped to the new session. Existing viewers retain
    /// their own handles, so input, frames, recovery and cancellation stay local.
    pub fn connect_request(&self, request: ConnectionRequest) -> Result<Self> {
        let mut sessions = self.client_sessions.lock().unwrap();
        sessions.retain(sessions::ClientSlot::is_live);
        let target = sessions::ClientTarget::new(&request);
        if sessions.iter().any(|slot| slot.target.overlaps(&target)) {
            anyhow::bail!(t!("err.session_in_progress").to_string());
        }
        let identity = if request.protocol == ConnectionProtocol::Removent {
            Some(self.identity()?)
        } else {
            None
        };
        let paths = self.paths.clone();
        let settings = self.settings.lock().unwrap().clone();
        let mut client = self.clone();
        client.client_task = Arc::new(Mutex::new(None));
        client.client_channels = Arc::new(Mutex::new(ClientChannels::default()));
        let (generation, recovery) = {
            let mut channels = client.client_channels.lock().unwrap();
            let generation = channels.invalidate();
            (generation, channels.recovery.clone())
        };
        let attempt = ClientAttempt {
            generation,
            recovery: recovery.clone(),
            started: std::time::Instant::now(),
            channels: client.client_channels.clone(),
            events: self.events_tx.clone(),
            sessions: Arc::downgrade(&self.client_sessions),
        };
        // Record only the destination and protocol, never the request/credentials.
        let span = tracing::info_span!("client_connection", pid = std::process::id(), attempt = attempt.generation,
            protocol = ?request.protocol, destination = %request.address);
        tracing::info!(parent: &span, "connection requested");
        let handle = self.rt.spawn(
            async move {
                // catch_unwind: even a task panic must reset the UI (the connect button
                // must not get stuck on "connecting…").
                let mut changes = recovery.subscribe();
                let monitor = async {
                    while changes.changed().await.is_ok() {
                        let state = changes.borrow_and_update().clone();
                        if state != removent_client::recovery::State::Connected {
                            attempt.pause_input();
                        }
                        let _ = attempt.events.send(UiEvent::ClientRecovery {
                            generation: attempt.generation,
                        });
                    }
                };
                let run = std::panic::AssertUnwindSafe(recovery.run(|| {
                    attempt.pause_input();
                    let request = recovery
                        .destination
                        .lock()
                        .unwrap()
                        .clone()
                        .unwrap_or_else(|| request.clone());
                    run_requested_client(
                        identity.clone(),
                        request,
                        paths.clone(),
                        settings.clone(),
                        attempt.clone(),
                    )
                }))
                .catch_unwind();
                let result = tokio::select! {
                    result = run => result,
                    _ = monitor => unreachable!(),
                };
                attempt.finish(match result {
                    Ok(Ok(())) => None,
                    Ok(Err(e)) => Some(format!("{e:#}")),
                    Err(_) => Some(t!("session.internal_error").to_string()),
                });
            }
            .instrument(span),
        );
        *client.client_task.lock().unwrap() = Some(handle);
        sessions.push(sessions::ClientSlot {
            generation,
            target,
            task: client.client_task.clone(),
        });
        Ok(client)
    }

    pub fn client_recovery(&self) -> removent_client::recovery::Recovery {
        self.client_channels.lock().unwrap().recovery.clone()
    }

    pub fn client_input_ready(&self, generation: usize) -> bool {
        let channels = self.client_channels.lock().unwrap();
        channels.generation == generation
            && channels.geometry.is_some()
            && channels.cmd.is_some()
            && channels.recovery.state() == removent_client::recovery::State::Connected
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
