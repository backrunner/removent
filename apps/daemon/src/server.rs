//! UDS IPC service: binds daemon_socket(), per-connection request/response plus
//! event-stream push.

use removent_core::Settings;
use removent_core::ipc::{IpcRequest, IpcResponse, read_msg, write_msg};
use rust_i18n::t;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast;

use crate::state::DaemonState;

/// Listen on and serve IPC connections until the shutdown token is cancelled;
/// removes the socket file on exit.
pub async fn serve(state: Arc<DaemonState>) -> anyhow::Result<()> {
    let sock = state.paths.daemon_socket();
    // A previous abnormal exit may have left a stale socket behind.
    let _ = std::fs::remove_file(&sock);
    if let Some(parent) = sock.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = UnixListener::bind(&sock)?;
    // Only this user may connect (the socket carries sensitive operations such as
    // admission arbitration).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600))?;
    }
    tracing::info!(path=%sock.display(), "IPC listening");

    loop {
        tokio::select! {
            _ = state.shutdown.cancelled() => break,
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _)) => {
                        let st = state.clone();
                        tokio::spawn(handle_conn(st, stream));
                    }
                    Err(e) => {
                        tracing::warn!(err=%e, "IPC accept failed");
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                }
            }
        }
    }
    let _ = std::fs::remove_file(&sock);
    Ok(())
}

async fn handle_conn(state: Arc<DaemonState>, stream: UnixStream) {
    state.tray_connections.fetch_add(1, Ordering::SeqCst);
    let (r, w) = stream.into_split();
    let mut r = tokio::io::BufReader::new(r);
    let w = Arc::new(tokio::sync::Mutex::new(w));

    // Event forwarding: broadcast → this connection (shares the write half with
    // responses, serialized by the Mutex).
    let mut ev_rx = state.events.subscribe();
    let w_ev = w.clone();
    let forwarder = tokio::spawn(async move {
        loop {
            match ev_rx.recv().await {
                Ok(ev) => {
                    let mut g = w_ev.lock().await;
                    if write_msg(&mut *g, &ev).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(skipped=%n, "IPC client lagged on events");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    while let Ok(Some(req)) = read_msg::<_, IpcRequest>(&mut r).await {
        let resp = handle_request(&state, req);
        let mut g = w.lock().await;
        if write_msg(&mut *g, &resp).await.is_err() {
            break;
        }
        if matches!(resp, IpcResponse::Ok) && state.shutdown.is_cancelled() {
            // Shutdown has been acknowledged; just disconnect.
            break;
        }
    }
    forwarder.abort();
    state.tray_connections.fetch_sub(1, Ordering::SeqCst);
}

fn handle_request(state: &Arc<DaemonState>, req: IpcRequest) -> IpcResponse {
    match req {
        IpcRequest::Status => IpcResponse::Status(Box::new(state.snapshot())),
        IpcRequest::PairingShow => {
            if let Some(code) = state.active_pin() {
                let expires_at_unix = (code.len() == 12)
                    .then(|| {
                        removent_core::pairing_invitation::Invitation::load(&state.paths)
                            .ok()
                            .flatten()
                            .map(|i| i.expires_at_unix)
                    })
                    .flatten();
                IpcResponse::PairingCode {
                    code,
                    expires_at_unix,
                }
            } else {
                IpcResponse::Error {
                    message: t!("error.no_pairing_code").to_string(),
                }
            }
        }
        IpcRequest::PairingGenerate => {
            let settings = state.settings.lock().unwrap();
            if settings.authentication.mode != removent_core::AuthenticationMode::PairingCode
                || settings.paired_only
                || settings.admission == removent_core::settings::AdmissionMode::DenyAll
            {
                return IpcResponse::Error {
                    message: t!("error.pairing_disabled").to_string(),
                };
            }
            if !state.enabled.load(Ordering::SeqCst) || !state.sessions.lock().unwrap().is_empty() {
                return IpcResponse::Error {
                    message: t!("error.pairing_busy").to_string(),
                };
            }
            // Serialize generation with authentication changes and service disable.
            match removent_core::pairing_invitation::Invitation::generate(&state.paths) {
                Ok(invite) => {
                    let code = invite.code().expose().to_owned();
                    state.show_pin(code.clone());
                    state.enabled_watch.send_modify(|_| {});
                    IpcResponse::PairingCode {
                        code,
                        expires_at_unix: Some(invite.expires_at_unix),
                    }
                }
                Err(e) => IpcResponse::Error {
                    message: e.to_string(),
                },
            }
        }
        IpcRequest::PairingRevoke => {
            match removent_core::pairing_invitation::Invitation::revoke(&state.paths) {
                Ok(()) => {
                    state.clear_pin();
                    // Do not interrupt an established session to remove an already-used code.
                    if state.sessions.lock().unwrap().is_empty() {
                        state.enabled_watch.send_modify(|_| {});
                    }
                    IpcResponse::Ok
                }
                Err(e) => IpcResponse::Error {
                    message: e.to_string(),
                },
            }
        }
        IpcRequest::RequestPermissions => {
            // Login startup never prompts. An explicit setup action runs in
            // the actual launchd process, keeping TCC attribution consistent.
            tokio::task::spawn_blocking(|| {
                if !crate::tcc::screen_recording_granted() {
                    crate::tcc::request_screen_recording();
                }
                if !crate::tcc::accessibility_granted() {
                    crate::tcc::request_accessibility();
                }
            });
            IpcResponse::Ok
        }
        IpcRequest::SetEnabled { on } => match state.set_enabled(on) {
            Ok(()) => IpcResponse::Ok,
            Err(e) => IpcResponse::Error {
                message: e.to_string(),
            },
        },
        IpcRequest::ReloadSettings => match Settings::load(&state.paths) {
            Ok(mut s) => {
                if state.login_window {
                    crate::login_window::restrict(&mut s);
                }
                let mut current = state.settings.lock().unwrap();
                let restart = current.authentication != s.authentication
                    || current.admission != s.admission
                    || current.paired_only != s.paired_only;
                *current = s;
                if restart {
                    let _ = removent_core::pairing_invitation::Invitation::revoke(&state.paths);
                    state.clear_pin();
                    removent_host::session::clear_resume_registry();
                    state.enabled_watch.send_modify(|_| {});
                }
                IpcResponse::Ok
            }
            Err(e) => IpcResponse::Error {
                message: t!("error.reload_settings", err = format!("{e:#}")).to_string(),
            },
        },
        IpcRequest::AdmissionReply { request_id, allow } => {
            if state.reply_admission(request_id, allow) {
                IpcResponse::Ok
            } else {
                IpcResponse::Error {
                    message: t!("error.unknown_request_id", id = request_id).to_string(),
                }
            }
        }
        IpcRequest::KickSession { session_id } => IpcResponse::Error {
            message: t!("error.kick_unimplemented", id = session_id).to_string(),
        },
        IpcRequest::Shutdown => {
            if let Err(error) = removent_core::service_intent::set_stopped(&state.paths, true) {
                return IpcResponse::Error {
                    message: error.to_string(),
                };
            }
            state.shutdown.cancel();
            IpcResponse::Ok
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authentication_reload_notifies_runner_without_changing_host_switch() {
        let dir = tempfile::tempdir().unwrap();
        let paths = removent_core::DataPaths {
            root: dir.path().into(),
        };
        let settings = Settings {
            host_enabled: false,
            ..Default::default()
        };
        settings.save(&paths).unwrap();
        let state = Arc::new(DaemonState::new(paths.clone(), settings, "test".into()));
        let mut changes = state.enabled_watch.subscribe();
        Settings::update(&paths, |s| {
            s.authentication.mode = removent_core::AuthenticationMode::None
        })
        .unwrap();
        assert!(matches!(
            handle_request(&state, IpcRequest::ReloadSettings),
            IpcResponse::Ok
        ));
        assert!(changes.has_changed().unwrap());
        assert!(!*changes.borrow_and_update());
        assert!(!state.enabled.load(Ordering::SeqCst));
        assert_eq!(
            state.settings.lock().unwrap().authentication.mode,
            removent_core::AuthenticationMode::None
        );
        Settings::update(&paths, |s| s.device_name = "New name".into()).unwrap();
        assert!(matches!(
            handle_request(&state, IpcRequest::ReloadSettings),
            IpcResponse::Ok
        ));
        assert!(!changes.has_changed().unwrap());
    }
}

#[cfg(test)]
mod invitation_tests {
    use super::*;
    #[test]
    fn cli_invitation_lifecycle_and_host_policy_are_enforced() {
        let dir = tempfile::tempdir().unwrap();
        let paths = removent_core::DataPaths {
            root: dir.path().into(),
        };
        let settings = Settings {
            host_enabled: true,
            admission: removent_core::AdmissionMode::TrustedAuto,
            ..Default::default()
        };
        settings.save(&paths).unwrap();
        let state = Arc::new(DaemonState::new(paths, settings, "test".into()));
        let code = match handle_request(&state, IpcRequest::PairingGenerate) {
            IpcResponse::PairingCode {
                code,
                expires_at_unix: Some(_),
            } => code,
            _ => panic!("expected generated code"),
        };
        assert_eq!(code.len(), 12);
        state.show_pin("654321".into());
        assert!(matches!(handle_request(&state, IpcRequest::PairingShow),
            IpcResponse::PairingCode { code, .. } if code == "654321"));
        state.clear_reactive_pin();
        assert_eq!(state.active_pin().as_deref(), Some(code.as_str()));
        // Survives daemon restart even without an in-memory display PIN.
        *state.pending_pin.lock().unwrap() = None;
        assert_eq!(state.active_pin().as_deref(), Some(code.as_str()));

        assert!(
            matches!(handle_request(&state, IpcRequest::PairingShow), IpcResponse::PairingCode { code: shown, .. } if shown == code)
        );
        assert!(matches!(
            handle_request(&state, IpcRequest::PairingRevoke),
            IpcResponse::Ok
        ));
        assert!(matches!(
            handle_request(&state, IpcRequest::PairingShow),
            IpcResponse::Error { .. }
        ));
        state.show_pin("123456".into());
        assert!(
            matches!(handle_request(&state, IpcRequest::PairingShow), IpcResponse::PairingCode { code, .. } if code == "123456")
        );
        let mut events = state.events.subscribe();
        state.clear_reactive_pin();
        assert!(matches!(
            events.try_recv(),
            Ok(removent_core::ipc::IpcEvent::PairingCleared)
        ));
        assert!(matches!(
            handle_request(&state, IpcRequest::PairingGenerate),
            IpcResponse::PairingCode { .. }
        ));
        state.set_enabled(false).unwrap();
        assert_eq!(state.active_pin(), None);
        assert!(
            removent_core::pairing_invitation::Invitation::load(&state.paths)
                .unwrap()
                .is_none()
        );
        state.set_enabled(true).unwrap();
        assert_eq!(state.active_pin(), None);
        state.settings.lock().unwrap().authentication.mode =
            removent_core::AuthenticationMode::None;
        assert!(matches!(
            handle_request(&state, IpcRequest::PairingGenerate),
            IpcResponse::Error { .. }
        ));
    }
}
