//! UDS IPC integration tests: status / set_enabled / admission reply and timeout.

use removent_core::ipc::{IpcEvent, IpcRequest, IpcResponse, connect, read_msg, write_msg};
use removent_core::{DataPaths, Settings};
use removent_daemon::{server, state::DaemonState};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

struct TestDaemon {
    state: Arc<DaemonState>,
    paths: DataPaths,
    _tmp: tempfile::TempDir,
    server: tokio::task::JoinHandle<()>,
}

async fn spawn_daemon_with(admission_timeout: Option<Duration>) -> TestDaemon {
    let tmp = tempfile::tempdir().unwrap();
    let paths = DataPaths {
        root: tmp.path().to_path_buf(),
    };
    paths.ensure_layout().unwrap();
    let settings = Settings {
        host_enabled: true,
        ..Settings::default()
    };
    let mut state = DaemonState::new(paths.clone(), settings, "a1b2c3d4".into());
    if let Some(t) = admission_timeout {
        state.admission_timeout = t;
    }
    let state = Arc::new(state);
    let server = tokio::spawn({
        let st = state.clone();
        async move {
            server::serve(st).await.unwrap();
        }
    });
    // Wait for the socket to appear before connecting, avoiding a startup race.
    for _ in 0..100 {
        if paths.daemon_socket().exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    TestDaemon {
        state,
        paths,
        _tmp: tmp,
        server,
    }
}

struct Client {
    r: tokio::io::BufReader<OwnedReadHalf>,
    w: OwnedWriteHalf,
}

impl Client {
    async fn connect(paths: &DataPaths) -> Self {
        let (r, w) = connect(paths).await.unwrap();
        Self { r, w }
    }

    async fn request(&mut self, req: IpcRequest) -> IpcResponse {
        write_msg(&mut self.w, &req).await.unwrap();
        // Events may interleave with responses; skip events and take the first response.
        loop {
            let v: serde_json::Value = read_msg(&mut self.r)
                .await
                .unwrap()
                .expect("daemon closed the connection");
            match v.get("type").and_then(serde_json::Value::as_str) {
                Some("status" | "ok" | "error") => {
                    return serde_json::from_value(v).unwrap();
                }
                _ => continue,
            }
        }
    }

    /// Wait until the daemon-side event forwarding task has completed its broadcast
    /// subscription (otherwise a broadcast right after would be lost).
    async fn wait_event_ready(&self, d: &TestDaemon, min_receivers: usize) {
        for _ in 0..100 {
            if d.state.events.receiver_count() >= min_receivers {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("daemon event subscription not ready");
    }

    /// Read the next event (non-event messages panic, which is acceptable inside tests).
    async fn next_event(&mut self) -> IpcEvent {
        loop {
            let v: serde_json::Value = read_msg(&mut self.r)
                .await
                .unwrap()
                .expect("daemon closed the connection");
            match v.get("type").and_then(serde_json::Value::as_str) {
                Some("status" | "ok" | "error") => continue,
                _ => return serde_json::from_value(v).unwrap(),
            }
        }
    }
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        self.state.shutdown.cancel();
        self.server.abort();
    }
}

#[tokio::test]
async fn status_roundtrip() {
    let d = spawn_daemon_with(None).await;
    let mut c = Client::connect(&d.paths).await;
    match c.request(IpcRequest::Status).await {
        IpcResponse::Status(report) => {
            assert!(report.running);
            assert_eq!(report.port, removent_proto::DEFAULT_PORT);
            assert_eq!(report.fp_short, "a1b2c3d4");
            assert!(report.sessions.is_empty());
            assert!(report.pending_pin.is_none());
        }
        other => panic!("expected Status, got {other:?}"),
    }
}

#[tokio::test]
async fn set_enabled_toggles_and_broadcasts() {
    let d = spawn_daemon_with(None).await;
    let mut c = Client::connect(&d.paths).await;

    // Open a pure event connection first to observe StateChanged.
    let mut watcher = Client::connect(&d.paths).await;
    watcher.wait_event_ready(&d, 2).await;

    assert!(matches!(
        c.request(IpcRequest::SetEnabled { on: false }).await,
        IpcResponse::Ok
    ));
    assert!(!d.state.enabled.load(std::sync::atomic::Ordering::SeqCst));
    assert!(!Settings::load(&d.paths).unwrap().host_enabled);
    let restarted = DaemonState::new(
        d.paths.clone(),
        Settings::load(&d.paths).unwrap(),
        "a1b2c3d4".into(),
    );
    assert!(!restarted.snapshot().running);

    match watcher.next_event().await {
        IpcEvent::StateChanged { running } => assert!(!running),
        other => panic!("expected StateChanged, got {other:?}"),
    }

    match c.request(IpcRequest::Status).await {
        IpcResponse::Status(report) => assert!(!report.running),
        other => panic!("expected Status, got {other:?}"),
    }
}

#[tokio::test]
async fn failed_switch_save_returns_error_and_preserves_running_state() {
    let d = spawn_daemon_with(None).await;
    std::fs::create_dir(d.paths.settings_file()).unwrap();
    let mut c = Client::connect(&d.paths).await;
    assert!(matches!(
        c.request(IpcRequest::SetEnabled { on: false }).await,
        IpcResponse::Error { .. }
    ));
    assert!(d.state.snapshot().running);
}

#[tokio::test]
async fn repeated_enable_does_not_restart_host_and_preserves_other_settings() {
    let d = spawn_daemon_with(None).await;
    let settings = Settings {
        device_name: "Updated by app".into(),
        ..Settings::default()
    };
    settings.save(&d.paths).unwrap();
    let mut watch = d.state.enabled_watch.subscribe();
    watch.borrow_and_update();
    let mut c = Client::connect(&d.paths).await;
    assert!(matches!(
        c.request(IpcRequest::SetEnabled { on: true }).await,
        IpcResponse::Ok
    ));
    assert!(!watch.has_changed().unwrap());
    assert_eq!(
        Settings::load(&d.paths).unwrap().device_name,
        "Updated by app"
    );
}

#[tokio::test]
async fn admission_reply_resolves() {
    let d = spawn_daemon_with(None).await;
    let mut c = Client::connect(&d.paths).await;
    c.wait_event_ready(&d, 1).await;

    // The daemon side initiates admission (simulating the runner's admission_prompt).
    let st = d.state.clone();
    let pending = tokio::spawn(async move {
        st.request_admission("Living Room Mac".into(), "0123456789abcdef".into())
            .await
    });

    let request_id = match c.next_event().await {
        IpcEvent::AdmissionRequest {
            request_id,
            peer_name,
            peer_fp16,
        } => {
            assert_eq!(peer_name, "Living Room Mac");
            assert_eq!(peer_fp16, "0123456789abcdef");
            request_id
        }
        other => panic!("expected AdmissionRequest, got {other:?}"),
    };

    assert!(matches!(
        c.request(IpcRequest::AdmissionReply {
            request_id,
            allow: true
        })
        .await,
        IpcResponse::Ok
    ));
    assert!(pending.await.unwrap());

    match c.next_event().await {
        IpcEvent::AdmissionResolved {
            request_id: id,
            allow,
        } => {
            assert_eq!(id, request_id);
            assert!(allow);
        }
        other => panic!("expected AdmissionResolved, got {other:?}"),
    }

    // Replying again with the same id: already consumed, should error.
    assert!(matches!(
        c.request(IpcRequest::AdmissionReply {
            request_id,
            allow: true
        })
        .await,
        IpcResponse::Error { .. }
    ));
}

#[tokio::test]
async fn admission_timeout_denies() {
    let d = spawn_daemon_with(Some(Duration::from_millis(150))).await;

    let allowed = d
        .state
        .request_admission("peer".into(), "0123456789abcdef".into())
        .await;
    assert!(!allowed, "no reply before the timeout should deny");
}

#[tokio::test]
async fn kick_session_unimplemented_and_shutdown() {
    let d = spawn_daemon_with(None).await;
    let mut c = Client::connect(&d.paths).await;
    assert!(matches!(
        c.request(IpcRequest::KickSession { session_id: 1 }).await,
        IpcResponse::Error { .. }
    ));
    assert!(matches!(
        c.request(IpcRequest::Shutdown).await,
        IpcResponse::Ok
    ));
    assert!(d.state.shutdown.is_cancelled());
}

#[tokio::test]
async fn cancelling_admission_resolves_prompt_and_rejects_late_reply() {
    let d = spawn_daemon_with(None).await;
    let mut events = d.state.events.subscribe();
    let state = d.state.clone();
    let task = tokio::spawn(async move {
        state
            .request_admission("peer".into(), "0123456789abcdef".into())
            .await
    });
    let IpcEvent::AdmissionRequest { request_id, .. } = events.recv().await.unwrap() else {
        panic!("expected admission request");
    };
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let event = tokio::time::timeout(Duration::from_secs(1), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(event, IpcEvent::AdmissionResolved { request_id: id, allow: false } if id == request_id)
    );
    assert!(!d.state.reply_admission(request_id, true));
}
