use super::*;
use removent_automation::{Executor, Outcome, Provider};
use removent_proto::automation::{Command, Failure, Request};
use serde_json::json;
use std::sync::atomic::AtomicBool;
use tokio_util::sync::CancellationToken;

struct Fake {
    executed: Arc<AtomicUsize>,
    held: Arc<AtomicBool>,
}
impl Provider for Fake {
    fn execute(&mut self, command: &Command, cancel: &CancellationToken) -> Outcome {
        if let Command::PressKey { .. } = command {
            self.held.store(true, Ordering::SeqCst);
            while !cancel.is_cancelled() {
                std::thread::sleep(Duration::from_millis(5));
            }
            std::thread::sleep(Duration::from_millis(30));
            self.held.store(false, Ordering::SeqCst);
            return Err(Failure::new("cancelled", "released"));
        }
        Ok(json!({"executed":self.executed.fetch_add(1,Ordering::SeqCst)+1}))
    }
}
struct ObservedInput {
    recorder: removent_host::RecorderInputSink,
    native: Option<removent_host::RealInputSink>,
    held: Arc<AtomicBool>,
    overlap: AtomicBool,
}
impl removent_host::InputSink for ObservedInput {
    fn mouse(
        &self,
        d: u64,
        x: f32,
        y: f32,
        b: u8,
        k: removent_proto::MouseKind,
    ) -> Result<(), String> {
        if self.held.load(Ordering::SeqCst) {
            self.overlap.store(true, Ordering::SeqCst);
        }
        self.recorder.mouse(d, x, y, b, k)?;
        self.native
            .as_ref()
            .map_or(Ok(()), |native| native.mouse(d, x, y, b, k))
    }
    fn key(
        &self,
        v: u16,
        m: removent_proto::KeyModifiers,
        k: removent_proto::KeyKind,
        c: Option<char>,
    ) -> Result<(), String> {
        if self.held.load(Ordering::SeqCst) {
            self.overlap.store(true, Ordering::SeqCst);
        }
        self.recorder.key(v, m, k, c)?;
        self.native
            .as_ref()
            .map_or(Ok(()), |native| native.key(v, m, k, c))
    }
    fn scroll(&self, d: u64, x: f32, y: f32, p: removent_proto::ScrollPhase) -> Result<(), String> {
        self.recorder.scroll(d, x, y, p)?;
        self.native
            .as_ref()
            .map_or(Ok(()), |native| native.scroll(d, x, y, p))
    }
}
struct Fixture {
    executor: Option<Executor>,
    input: Arc<ObservedInput>,
    session: removent_client::ClientSession,
    host: tokio::task::JoinHandle<()>,
    executed: Arc<AtomicUsize>,
    held: Arc<AtomicBool>,
    clip: Arc<removent_core::MemoryClipboard>,
    paths: DataPaths,
    _dir: tempfile::TempDir,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.session
            .conn
            .inner()
            .close(0u32.into(), b"fixture finished");
        self.host.abort();
    }
}
async fn fixture(allowed: bool) -> Fixture {
    fixture_with_native(allowed, false).await
}
async fn fixture_with_native(allowed: bool, native: bool) -> Fixture {
    let (client_id, cd) = identity_for("AutomationClient");
    let (host_id, hd) = identity_for("AutomationHost");
    let (server, _) = make_server_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &host_id,
        PinState::new([client_id.fingerprint], false),
    )
    .unwrap();
    let address = server.local_addr().unwrap();
    let (client, _) = make_client_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &client_id,
        PinState::new([host_id.fingerprint], false),
    )
    .unwrap();
    let (pin_tx, pin_rx) = oneshot::channel();
    let (executor_tx, executor_rx) = oneshot::channel();
    let executed = Arc::new(AtomicUsize::new(0));
    let held = Arc::new(AtomicBool::new(false));
    let fake = Fake {
        executed: executed.clone(),
        held: held.clone(),
    };
    let input = Arc::new(ObservedInput {
        recorder: Default::default(),
        native: native.then(removent_host::RealInputSink::new),
        held: held.clone(),
        overlap: AtomicBool::new(false),
    });
    let host_input = input.clone();
    let clip = removent_core::MemoryClipboard::new();
    let host_clip = clip.clone();
    let host = tokio::spawn(async move {
        let _hd = hd;
        let conn = RvpConnection::new(server.accept().await.unwrap().await.unwrap());
        let cfg = HostConfig {
            authentication: removent_core::AuthenticationSettings {
                mode: removent_core::AuthenticationMode::PairingCode,
                ..Default::default()
            },
            auth_paths: None,
            audio_available: false,
            preapproved_only: false,
            device_name: "Host".into(),
            admission: AdmissionMode::AlwaysAsk,
            video_bitrate_kbps: 3000,
            video_fps: 30,
            input_sink: Some(host_input),
            local_clip: Some(host_clip.clone()),
        };
        let (established, _, _, commands, sink, source) = serve_connection(
            conn.clone(),
            &host_id,
            &mut PeersStore::in_memory(),
            &cfg,
            HostInteractions::auto_allow_with_pin_tx(pin_tx),
            removent_proto::DisplayInfo {
                id: 1,
                w_px: 320,
                h_px: 240,
                scale: 1.,
                dpi: 96,
                is_main: true,
            },
        )
        .await
        .unwrap();
        let executor = allowed.then(|| {
            if native {
                Executor::native(established.cancel.clone()).unwrap()
            } else {
                Executor::spawn(established.cancel.clone(), move || (fake, ())).unwrap()
            }
        });
        let _ = executor_tx.send(executor.clone());
        removent_host::spawn_control_pump(
            source,
            sink,
            removent_host::ControlPumpDeps {
                automation: executor,
                conn,
                kf_tx: None,
                controller: None,
                window_ms: 250,
                input: cfg.input_sink,
                local_clip: Some(host_clip),
                quality_tx: None,
                caps: established.peer_caps,
                clip_state: established.clip_state,
                cancel: established.cancel,
                peer_fp: None,
                delivery: None,
            },
            commands,
        )
        .await
        .unwrap();
    });
    let session = connect_session(
        client,
        address,
        &client_id,
        ClientConfig {
            native_video: false,
            video_quality: Default::default(),
            host_identity_verified: true,
            pairing_code: None,
            device_name: "Client".into(),
            caps: Caps {
                audio: false,
                ..Caps::all()
            },
            local_clip: None,
        },
        None,
        None,
        Some(Box::new(move |_, sender| {
            tokio::spawn(async move {
                let _ = sender.send(pin_rx.await.unwrap());
            });
        })),
    )
    .await
    .unwrap();
    assert_ne!(
        session.host_features & removent_proto::feature_bits::AUTOMATION,
        0
    );
    let paths = DataPaths {
        root: cd.path().to_owned(),
    };
    Fixture {
        executor: executor_rx.await.unwrap(),
        input,
        session,
        host,
        executed,
        held,
        clip,
        paths,
        _dir: cd,
    }
}
fn request(id: &str, command: Command) -> Request {
    Request {
        control_revision: None,
        version: 1,
        operation_id: id.into(),
        command,
    }
}
async fn wait_for(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn automation_auth_ack_dedup_clipboard_and_stalled_stream_are_independent() {
    let f = fixture(true).await;
    // An incomplete header must not monopolize the connection's sole bidi acceptor.
    let (mut stalled, _recv) = f.session.conn.inner().open_bi().await.unwrap();
    stalled.write_all(b"C").await.unwrap();
    let req = request("one", Command::TypeText { text: "x".into() });
    for _ in 0..2 {
        assert_eq!(
            removent_net::automation::call(&f.session.conn, &req)
                .await
                .unwrap()
                .result
                .unwrap()["executed"],
            1
        );
    }
    assert_eq!(f.executed.load(Ordering::SeqCst), 1);
    let conflict = request("one", Command::TypeText { text: "y".into() });
    assert_eq!(
        removent_net::automation::call(&f.session.conn, &conflict)
            .await
            .unwrap()
            .result
            .unwrap_err()
            .code,
        "operation_conflict"
    );
    f.session
        .send_clipboard_text(1, "clipboard still works")
        .await
        .unwrap();
    use removent_core::TextClipboard;
    wait_for(|| f.clip.read().ok().as_deref() == Some("clipboard still works")).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn automation_requires_explicit_host_grant() {
    let f = fixture(false).await;
    let req = request("cap", Command::Capabilities {});
    assert_eq!(
        removent_net::automation::call(&f.session.conn, &req)
            .await
            .unwrap()
            .result
            .unwrap_err()
            .code,
        "permission_denied"
    );
    assert_eq!(f.executed.load(Ordering::SeqCst), 0);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn desktop_broker_reuses_connection_and_cancels_remote_input_on_caller_drop() {
    use removent_client::automation::broker::{Attachment, Publisher};
    let f = fixture(true).await;
    let publisher =
        Publisher::start(&f.paths, f.session.conn.clone(), f.session.host_features).unwrap();
    let host = f.session.conn.peer_fingerprint().unwrap();
    let attached = Attachment::find(&f.paths, host).await.unwrap().unwrap();
    assert!(attached.alive().await);
    let req = request("first", Command::TypeText { text: "x".into() });
    assert!(attached.call(&req).await.unwrap().result.is_ok());
    let mut stale = attached.clone();
    stale.info.generation = "previous".into();
    assert!(!stale.alive().await);
    assert!(stale.call(&req).await.is_err());
    assert_eq!(f.executed.load(Ordering::SeqCst), 1);
    let held = request(
        "hold",
        Command::PressKey {
            key: "a".into(),
            hold_ms: 3000,
        },
    );
    let task = tokio::spawn(async move { attached.call(&held).await });
    wait_for(|| f.held.load(Ordering::SeqCst)).await;
    task.abort();
    let _ = task.await;
    wait_for(|| !f.held.load(Ordering::SeqCst)).await;
    // Dropping the broker must not close the desktop's connection.
    drop(publisher);
    assert!(f.session.conn.inner().close_reason().is_none());
    assert!(Attachment::find(&f.paths, host).await.unwrap().is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn viewer_takes_control_after_native_release_and_only_explicit_resume_allows_new_input() {
    use removent_client::automation::broker::{Attachment, Publisher};
    use removent_proto::{KeyKind, KeyModifiers};
    let f = fixture(true).await;
    let _publisher =
        Publisher::start(&f.paths, f.session.conn.clone(), f.session.host_features).unwrap();
    let attached = Attachment::find(&f.paths, f.session.conn.peer_fingerprint().unwrap())
        .await
        .unwrap()
        .unwrap();
    let operation = attached.clone();
    let task = tokio::spawn(async move {
        operation
            .call(&request(
                "holding",
                Command::PressKey {
                    key: "a".into(),
                    hold_ms: 3000,
                },
            ))
            .await
    });
    wait_for(|| f.held.load(Ordering::SeqCst)).await;
    // Status bypasses the broker's action lease while that action is running.
    let status = attached
        .call(&request("during", Command::ControlStatus {}))
        .await
        .unwrap()
        .result
        .unwrap();
    assert_eq!(status["active_operation"], "holding");
    f.session
        .send_input_key(0, KeyModifiers::empty(), KeyKind::Down, Some('a'))
        .await
        .unwrap();
    let result = task.await.unwrap().unwrap().result.unwrap_err();
    assert_eq!(result.code, "control_changed");
    wait_for(|| !f.input.recorder.events.lock().unwrap().is_empty()).await;
    assert!(
        !f.input.overlap.load(Ordering::SeqCst),
        "viewer down preceded agent up"
    );
    let status = attached
        .call(&request("paused", Command::ControlStatus {}))
        .await
        .unwrap()
        .result
        .unwrap();
    assert_eq!(status["mode"], "manual");
    assert_eq!(status["input_held"], true);
    let rejected = attached
        .call(&request(
            "blocked",
            Command::TypeText {
                text: "blocked".into(),
            },
        ))
        .await
        .unwrap();
    assert_eq!(rejected.result.unwrap_err().code, "control_changed");
    assert_eq!(f.executed.load(Ordering::SeqCst), 0);
    f.session
        .send_input_key(0, KeyModifiers::empty(), KeyKind::Up, None)
        .await
        .unwrap();
    wait_for(|| f.input.recorder.events.lock().unwrap().len() == 2).await;
    tokio::time::sleep(Duration::from_millis(520)).await;
    let status = attached
        .call(&request("ready", Command::ControlStatus {}))
        .await
        .unwrap()
        .result
        .unwrap();
    assert_eq!(status["can_resume"], true);
    let resumed = attached
        .call(&request(
            "resume",
            Command::ResumeControl {
                revision: status["revision"].as_u64().unwrap(),
            },
        ))
        .await
        .unwrap()
        .result
        .unwrap();
    let mut fresh = request(
        "fresh",
        Command::TypeText {
            text: "fresh".into(),
        },
    );
    fresh.control_revision = resumed["revision"].as_u64();
    attached.call(&fresh).await.unwrap().result.unwrap();
    assert_eq!(f.executed.load(Ordering::SeqCst), 1);
    // A later observer cannot make a pre-takeover request current retroactively.
    let late = attached
        .call(&request("late", Command::TypeText { text: "old".into() }))
        .await
        .unwrap();
    assert_eq!(late.result.unwrap_err().code, "control_changed");
}

#[path = "automation/local.rs"]
mod local;
#[path = "automation/native.rs"]
mod native;
