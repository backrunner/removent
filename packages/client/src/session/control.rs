use super::*;

/// Build the session: media loops + control pump + clipboard poller.
pub(super) fn build_session(
    conn: RvpConnection,
    sink: ControlSink,
    mut source: ControlSource,
    ack: NegotiateAck,
    cfg: ClientConfig,
) -> Result<ClientSession, ConnectError> {
    let mut cfg = cfg;
    if !cfg.caps.clipboard {
        cfg.local_clip = None;
    }
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<ControlMsg>(64);
    let (input_tx, mut input_rx) = crate::input_queue::channel();
    let input_commands = cmd_tx.clone();
    let input_forwarder = tokio::spawn(async move {
        while let Some(event) = input_rx.recv().await {
            if input_commands.send(event.message).await.is_err() {
                break;
            }
            input_rx.record_sent(event.queued_at, true);
        }
    });
    let (decoded_bgra_tx, decoded_bgra_rx) = removent_core::latest::channel();
    let (decoded_pcm_tx, decoded_pcm_rx) = mpsc::channel(128);

    let resume_token = Arc::new(Mutex::new(ack.resume_token));
    let last_kf = Arc::new(Mutex::new(None));

    // Media receive: stream count follows the negotiation result (when audio.enabled is
    // off there is only a video stream).
    let dispatch = spawn_media_loops(
        conn.clone(),
        decoded_bgra_tx,
        decoded_pcm_tx,
        ack.audio.enabled,
        cmd_tx.clone(),
        last_kf.clone(),
    );

    // Session-level clipboard state: the pump and poller share the suppress singleton
    // to prevent echo loops.
    let clip_state = cfg
        .local_clip
        .as_ref()
        .map(|clip| Arc::new(ClipSyncState::new(clip.as_ref())));
    let local_clip = cfg.local_clip.clone();
    let last_quality = Arc::new(Mutex::new(None));

    let quality = last_quality.clone();
    let pump_clip_state = clip_state.clone();
    let pump_resume_token = resume_token.clone();
    // Local clipboard changes → send to the peer (shares suppress state with the pump).
    let poller = cfg.local_clip.clone().map(|clip| {
        removent_core::spawn_clipboard_poller(
            clip,
            clip_state.clone().expect("clip_state with clip"),
            cmd_tx.clone(),
            250,
        )
    });

    let (closed_tx, closed) = oneshot::channel();
    let cleanup = PumpCleanup {
        conn: conn.clone(),
        media: dispatch.abort_handle(),
        input: input_forwarder.abort_handle(),
        clipboard: poller.as_ref().map(|task| task.abort_handle()),
        closed: Some(closed_tx),
    };
    let clean_end = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let pump_clean_end = clean_end.clone();
    let clipboard_conn = conn.clone();
    let pump = tokio::spawn(async move {
        let _cleanup = cleanup;
        let (mut clipboard, clipboard_tx) =
            removent_net::clipboard::ClipboardReader::new(clipboard_conn);
        let mut writer =
            removent_net::control_writer::ControlWriter::with_clipboard(sink, clipboard_tx);
        let local_suppress = std::sync::atomic::AtomicU64::new(0);
        loop {
            tokio::select! {
                result = writer.progress(), if writer.has_pending() => {
                    if !matches!(result, Ok(false)) { break; }
                }
                item = clipboard.next(&mut source) => {
                    let Some(item) = item else { break };
                    let Ok(item) = item else { break };
                    match item {
                        ControlItem::Msg(m) => match *m {
                            ControlMsg::ClipboardSync { seq, format, data } => {
                                if format == removent_proto::ClipFormat::TextUtf8
                                    && let Some(clip) = local_clip.as_ref() {
                                    let suppress: &std::sync::atomic::AtomicU64 = pump_clip_state
                                        .as_ref()
                                        .map(|s| &s.suppress_cc)
                                        .unwrap_or(&local_suppress);
                                    if let Err(e) = removent_core::apply_incoming_clip(
                                        clip.as_ref(),
                                        suppress,
                                        &data,
                                    ) {
                                        tracing::warn!(err=%e, "clipboard apply failed");
                                    }
                                }
                                // An Ack confirms receipt, not that this endpoint had a
                                // local pasteboard bridge. Always acknowledge the message
                                // so a sender cannot remain blocked when clipboard support
                                // is disabled or unavailable on this side.
                                if writer.enqueue(ControlMsg::ClipboardAck { seq }).is_err() {
                                    break;
                                }
                            }
                            ControlMsg::Ping { ts_us } => {
                                if writer.enqueue(ControlMsg::Pong { ts_us }).is_err() {
                                    break;
                                }
                            }
                            ControlMsg::QualityControl {
                                bitrate_kbps,
                                fps,
                                scale,
                                ..
                            } => {
                                tracing::info!(bitrate_kbps, "host quality control");
                                *quality.lock().unwrap() = Some((bitrate_kbps, fps, scale));
                            }
                            // resume rotation (§7.4): the host sends back an ack with the new token after resume.
                            ControlMsg::NegotiateReply { ack } => {
                                if let Some(t) = ack.resume_token {
                                    *pump_resume_token.lock().unwrap() = Some(t);
                                }
                            }
                            ControlMsg::SessionEnd { .. } => {
                                pump_clean_end.store(true, std::sync::atomic::Ordering::SeqCst);
                                break;
                            },
                            _ => {}
                        },
                        ControlItem::Skipped => continue,
                    }
                }
                maybe_cmd = cmd_rx.recv(), if writer.accepts_commands() => {
                    match maybe_cmd {
                        Some(msg @ ControlMsg::SessionEnd { .. }) => {
                            pump_clean_end.store(true, std::sync::atomic::Ordering::SeqCst);
                            // Outbound SessionEnd must actually reach the peer before we exit.
                            if writer.enqueue(msg).is_err() { break; }
                        }
                        None => break,
                        Some(msg) => {
                            if writer.enqueue(msg).is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        }
    });

    let mut tasks = vec![dispatch, pump, input_forwarder];
    if let Some(p) = poller {
        tasks.push(p);
    }

    Ok(ClientSession {
        conn,
        cmd_tx,
        input_tx,
        resume_token,
        negotiated: ack,
        decoded_bgra_rx,
        decoded_pcm_rx,
        last_quality,
        last_kf,
        tasks,
        closed,
        clean_end,
    })
}

/// Teardown runs on EOF, SessionEnd, a send failure, panic or cancellation.
pub(super) struct PumpCleanup {
    pub(super) conn: RvpConnection,
    pub(super) media: tokio::task::AbortHandle,
    pub(super) input: tokio::task::AbortHandle,
    pub(super) clipboard: Option<tokio::task::AbortHandle>,
    pub(super) closed: Option<oneshot::Sender<()>>,
}

impl Drop for PumpCleanup {
    fn drop(&mut self) {
        self.media.abort();
        self.input.abort();
        if let Some(task) = &self.clipboard {
            task.abort();
        }
        self.conn
            .inner()
            .close(removent_net::quinn::VarInt::from_u32(0), b"session ended");
        if let Some(tx) = self.closed.take() {
            let _ = tx.send(());
        }
    }
}
