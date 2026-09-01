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
        IpcRequest::SetEnabled { on } => {
            state.set_enabled(on);
            IpcResponse::Ok
        }
        IpcRequest::ReloadSettings => match Settings::load(&state.paths) {
            Ok(s) => {
                *state.settings.lock().unwrap() = s;
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
            state.shutdown.cancel();
            IpcResponse::Ok
        }
    }
}
