use super::*;

pub(super) fn handle_daemon_message(
    v: serde_json::Value,
    events: &std::sync::mpsc::Sender<UiEvent>,
    running: &Arc<AtomicBool>,
    perms: &Arc<Mutex<Option<(bool, bool)>>>,
    sessions: &Arc<AtomicUsize>,
) {
    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match ty {
        "status" | "ok" | "error" => {
            match serde_json::from_value::<IpcResponse>(v) {
                Ok(IpcResponse::Status(report)) => {
                    apply_status(*report, events, running, perms, sessions)
                }
                // Daemon-side failures (e.g. a rejected request) must reach the UI
                // instead of being silently dropped.
                Ok(IpcResponse::Error { message }) => {
                    let _ = events.send(UiEvent::Notice(message));
                }
                _ => {}
            }
        }
        _ => {
            if let Ok(ev) = serde_json::from_value::<IpcEvent>(v) {
                match ev {
                    IpcEvent::StateChanged { running: on } => {
                        running.store(on, Ordering::SeqCst);
                        let _ = events.send(UiEvent::HostStateChanged(on));
                    }
                    IpcEvent::SessionStarted { session } => {
                        sessions.fetch_add(1, Ordering::SeqCst);
                        let _ = events.send(UiEvent::HostSessionStarted {
                            peer_name: session.peer_name,
                            codec: session.video_codec,
                        });
                    }
                    IpcEvent::SessionEnded { reason, .. } => {
                        let _ = sessions.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                            Some(n.saturating_sub(1))
                        });
                        let _ = events.send(UiEvent::HostSessionEnded(reason));
                    }
                    IpcEvent::AdmissionRequest {
                        request_id,
                        peer_name,
                        peer_fp16,
                    } => {
                        let _ = events.send(UiEvent::AdmissionRequest {
                            request_id,
                            peer_name,
                            peer_fp_short: peer_fp16,
                        });
                    }
                    IpcEvent::AdmissionResolved { request_id, allow } => {
                        let _ = events.send(UiEvent::AdmissionResolved { request_id, allow });
                    }
                    IpcEvent::PairingPin { pin } => {
                        let _ = events.send(UiEvent::PairingPin(pin));
                    }
                    IpcEvent::PairingCleared => {
                        let _ = events.send(UiEvent::PairingCleared);
                    }
                    IpcEvent::PairingDone { peer_name } => {
                        let _ = events.send(UiEvent::PairingDone(peer_name));
                    }
                }
            }
        }
    }
}

pub(super) fn apply_status(
    report: StatusReport,
    events: &std::sync::mpsc::Sender<UiEvent>,
    running: &Arc<AtomicBool>,
    perms: &Arc<Mutex<Option<(bool, bool)>>>,
    sessions: &Arc<AtomicUsize>,
) {
    let prev = running.swap(report.running, Ordering::SeqCst);
    if prev != report.running {
        let _ = events.send(UiEvent::HostStateChanged(report.running));
    }
    // Reconcile the session count with the daemon's authoritative list: the
    // SessionStarted/Ended increments alone drift when the app starts with
    // sessions already running or the UDS link drops and reconnects, and a
    // false "no session" would let an update install kill a live session.
    sessions.store(report.sessions.len(), Ordering::SeqCst);
    // Notify only on change: status is polled every few seconds.
    let snap = (
        report.screen_recording_granted,
        report.accessibility_granted,
    );
    let mut slot = perms.lock().unwrap();
    if *slot != Some(snap) {
        *slot = Some(snap);
        let _ = events.send(UiEvent::DaemonPermissions {
            screen_recording: snap.0,
            accessibility: snap.1,
        });
    }
}
