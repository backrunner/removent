use super::*;
use removent_proto::automation::{Button, Point, TextPlacement};
use serde_json::Value;
use std::{
    path::PathBuf,
    process::{Child, Command as Process},
    time::Instant,
};

struct Receiver {
    process: Child,
    directory: tempfile::TempDir,
    ready: Value,
}
impl Drop for Receiver {
    fn drop(&mut self) {
        let _ = std::fs::write(self.directory.path().join("quit"), []);
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if self.process.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}
impl Receiver {
    async fn command(&self, operation: &str) {
        let id = IDS.fetch_add(1, Ordering::SeqCst).to_string();
        let temporary = self.directory.path().join("command.tmp");
        std::fs::write(
            &temporary,
            serde_json::to_vec(&json!({"id":id,"operation":operation})).unwrap(),
        )
        .unwrap();
        std::fs::rename(temporary, self.directory.path().join("command.json")).unwrap();
        wait_for(|| {
            std::fs::read(self.directory.path().join("reply.json"))
                .ok()
                .and_then(|data| serde_json::from_slice::<Value>(&data).ok())
                .is_some_and(|reply| {
                    if reply["id"] != id {
                        return false;
                    }
                    assert_eq!(reply["result"], "ok", "{reply}");
                    true
                })
        })
        .await;
    }
    async fn start() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("receiver");
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../scripts/qa/automation_receiver.swift");
        let compiled = Process::new("xcrun")
            .args(["swiftc"])
            .arg(source)
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let process = Process::new(executable)
            .arg(directory.path())
            .spawn()
            .unwrap();
        let mut receiver = Self {
            process,
            directory,
            ready: Value::Null,
        };
        tokio::time::timeout(Duration::from_secs(6), async {
            while !receiver.directory.path().join("ready.json").exists() {
                assert!(receiver.process.try_wait().unwrap().is_none(), "Native fixture exited before becoming ready; an unlocked macOS desktop is required");
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        receiver.ready = serde_json::from_slice(
            &std::fs::read(receiver.directory.path().join("ready.json")).unwrap(),
        )
        .unwrap();
        assert!(
            receiver.ready["window_number"].as_i64().unwrap() > 0,
            "{}",
            receiver.ready
        );
        assert_eq!(receiver.ready["visible"], true);
        receiver
    }
    fn events(&self) -> Vec<Value> {
        std::fs::read_to_string(self.directory.path().join("events.jsonl"))
            .unwrap()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }
    fn target(&self) -> String {
        format!("app:{}", self.ready["pid"].as_u64().unwrap())
    }
}

#[path = "native/monitor.rs"]
mod monitor;
static IDS: AtomicUsize = AtomicUsize::new(0);
async fn rpc(f: &Fixture, command: Command, revision: Option<u64>) -> Result<Value, Failure> {
    let mut req = request(
        &format!("native-{}", IDS.fetch_add(1, Ordering::SeqCst)),
        command,
    );
    req.control_revision = revision;
    removent_net::automation::call(&f.session.conn, &req)
        .await
        .unwrap()
        .result
}
async fn native_ready(f: &Fixture) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let status = rpc(f, Command::ControlStatus {}, None).await.unwrap();
            let monitor = &status["local_input_monitor"];
            assert_ne!(
                monitor["state"], "unavailable",
                "Local input monitor unavailable: {monitor}"
            );
            if monitor["state"] == "ready" {
                assert_eq!(
                    status["mode"], "automation",
                    "Human input paused the fixture; do not resume over an active user"
                );
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}
async fn act(f: &Fixture, receiver: &Receiver, command: Command) -> Value {
    if command.mutates() {
        let state = rpc(f, Command::State {}, None).await.unwrap();
        assert!(
            state["apps"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["target"] == receiver.target() && a["active"] == true),
            "Fixture lost foreground; refusing input into another application. Fixture state: {:?}; remote active: {:?}",
            std::fs::read_to_string(receiver.directory.path().join("ui.json")),
            state["apps"]
                .as_array()
                .unwrap()
                .iter()
                .find(|a| a["target"] == receiver.target())
                .map(|a| &a["active"])
        );
    }
    rpc(f, command, None).await.unwrap()
}
fn element<'a>(tree: &'a Value, id: &str) -> &'a Value {
    tree["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["identifier"] == id)
        .unwrap_or_else(|| {
            panic!(
                "Missing fixture element {id}; roles: {:?}",
                tree["nodes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .take(24)
                    .map(|n| (&n["role"], &n["identifier"]))
                    .collect::<Vec<_>>()
            )
        })
}
async fn value(f: &Fixture, receiver: &Receiver, id: &str, expected: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let tree = rpc(
                f,
                Command::Snapshot {
                    target: receiver.target(),
                },
                None,
            )
            .await
            .unwrap();
            if element(&tree, id)["value"] == expected {
                return tree;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap()
}
/// Creates one temporary AppKit window, restores focus, and never edits user data.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires an unlocked macOS GUI, Accessibility, Input Monitoring and Screen Recording permissions"]
async fn native_automation_rvp_accessibility_changes_appkit() {
    assert!(removent_input::accessibility_trusted());
    let receiver = Receiver::start().await;
    let f = fixture_with_native(true, true).await;
    native_ready(&f).await;
    let caps = rpc(&f, Command::Capabilities {}, None).await.unwrap();
    assert_eq!(caps["screen"], true);
    assert_eq!(caps["native_ui"], true);
    let tree = rpc(
        &f,
        Command::Snapshot {
            target: receiver.target(),
        },
        None,
    )
    .await
    .unwrap();
    let field = element(&tree, "automation-text")["reference"]
        .as_str()
        .unwrap()
        .to_owned();
    act(
        &f,
        &receiver,
        Command::Focus {
            reference: field.clone(),
        },
    )
    .await;
    act(
        &f,
        &receiver,
        Command::SetValue {
            reference: field.clone(),
            value: "prefix 中文😀 suffix".into(),
        },
    )
    .await;
    act(
        &f,
        &receiver,
        Command::SelectText {
            reference: field.clone(),
            text: "中文😀".into(),
            prefix: Some("prefix ".into()),
            suffix: Some(" suffix".into()),
            placement: TextPlacement::Select,
        },
    )
    .await;
    act(
        &f,
        &receiver,
        Command::TypeText {
            text: "替换".into(),
        },
    )
    .await;
    let tree = value(&f, &receiver, "automation-text", "prefix 替换 suffix").await;
    let button = element(&tree, "automation-button")["reference"]
        .as_str()
        .unwrap()
        .to_owned();
    act(
        &f,
        &receiver,
        Command::ElementAction {
            reference: button,
            name: "AXPress".into(),
        },
    )
    .await;
    let tree = value(&f, &receiver, "automation-count", "1").await;
    let field = element(&tree, "automation-text")["reference"]
        .as_str()
        .unwrap()
        .to_owned();
    // Capture a current reference immediately before manual takeover.
    act(
        &f,
        &receiver,
        Command::Focus {
            reference: field.clone(),
        },
    )
    .await;
    f.session
        .send_input_key(
            0,
            removent_proto::KeyModifiers::empty(),
            removent_proto::KeyKind::Up,
            None,
        )
        .await
        .unwrap();
    wait_for(|| !f.input.recorder.events.lock().unwrap().is_empty()).await;
    tokio::time::sleep(Duration::from_millis(550)).await;
    let control = rpc(&f, Command::ControlStatus {}, None).await.unwrap();
    let resumed = rpc(
        &f,
        Command::ResumeControl {
            revision: control["revision"].as_u64().unwrap(),
        },
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        rpc(
            &f,
            Command::SetValue {
                reference: field,
                value: "never".into()
            },
            resumed["revision"].as_u64()
        )
        .await
        .unwrap_err()
        .code,
        "stale_reference"
    );

    eprintln!(
        "Native AX acceptance: press/value/focus, UTF-16 selection, Unicode replacement and stale references passed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires an unlocked macOS GUI, Accessibility, Input Monitoring and Screen Recording permissions"]
async fn native_automation_rvp_input_releases_keys_on_takeover() {
    assert!(removent_input::accessibility_trusted());
    let receiver = Receiver::start().await;
    let f = fixture_with_native(true, true).await;
    native_ready(&f).await;
    // The AppKit field starts focused and selected. Verify Unicode via its own UI state.
    act(
        &f,
        &receiver,
        Command::TypeText {
            text: "测试😀".into(),
        },
    )
    .await;
    wait_for(|| {
        std::fs::read(receiver.directory.path().join("ui.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .is_some_and(|ui| ui["text"] == "测试😀")
    })
    .await;
    let display_id = receiver.ready["display_id"].as_u64().unwrap();
    let screenshot = rpc(
        &f,
        Command::Screenshot {
            display_id,
            max_width: 1280,
        },
        None,
    )
    .await
    .unwrap();
    assert!(
        screenshot["data"]
            .as_str()
            .unwrap()
            .starts_with("iVBORw0KGgo")
    );
    let geometry = removent_input::DisplayGeometry::by_id(display_id).unwrap();
    let point = Point {
        observation: screenshot["observation"].as_u64().unwrap(),
        x: (receiver.ready["x"].as_f64().unwrap() - geometry.origin_x)
            * screenshot["width"].as_f64().unwrap()
            / geometry.width_pt,
        y: (receiver.ready["y"].as_f64().unwrap() - geometry.origin_y)
            * screenshot["height"].as_f64().unwrap()
            / geometry.height_pt,
    };
    act(
        &f,
        &receiver,
        Command::Click {
            point,
            button: Button::Left,
            count: 2,
            modifiers: vec![],
            hold_ms: 25,
        },
    )
    .await;
    act(
        &f,
        &receiver,
        Command::Click {
            point,
            button: Button::Right,
            count: 1,
            modifiers: vec!["shift".into()],
            hold_ms: 0,
        },
    )
    .await;
    act(
        &f,
        &receiver,
        Command::Drag {
            from: point,
            to: Point {
                x: point.x + 40.,
                ..point
            },
            duration_ms: 120,
        },
    )
    .await;
    act(
        &f,
        &receiver,
        Command::Scroll {
            point,
            dx: 0.,
            dy: 36.,
        },
    )
    .await;
    tokio::time::sleep(Duration::from_millis(80)).await;
    let before = receiver.events().len();
    let conn = f.session.conn.clone();
    let held = tokio::spawn(async move {
        removent_net::automation::call(
            &conn,
            &request(
                "native-held",
                Command::PressKey {
                    key: "Meta+Shift+A".into(),
                    hold_ms: 3000,
                },
            ),
        )
        .await
    });
    wait_for(|| {
        receiver
            .events()
            .iter()
            .skip(before)
            .any(|e| e["type"] == 10 && e["key"] == 0)
    })
    .await;
    // This is viewer input on the real control stream; the host must settle the native gesture first.
    f.session
        .send_input_key(
            0,
            removent_proto::KeyModifiers::empty(),
            removent_proto::KeyKind::Down,
            Some('a'),
        )
        .await
        .unwrap();
    assert_eq!(
        held.await.unwrap().unwrap().result.unwrap_err().code,
        "control_changed"
    );
    f.session
        .send_input_key(
            0,
            removent_proto::KeyModifiers::empty(),
            removent_proto::KeyKind::Up,
            None,
        )
        .await
        .unwrap();
    wait_for(|| {
        receiver
            .events()
            .iter()
            .skip(before)
            .filter(|e| e["type"] == 11 && e["key"] == 0)
            .count()
            >= 2
    })
    .await;
    tokio::time::sleep(Duration::from_millis(550)).await;
    let status = rpc(&f, Command::ControlStatus {}, None).await.unwrap();
    assert_eq!(status["mode"], "manual");
    let resumed = rpc(
        &f,
        Command::ResumeControl {
            revision: status["revision"].as_u64().unwrap(),
        },
        None,
    )
    .await
    .unwrap();
    let revision = resumed["revision"].as_u64();
    assert_eq!(
        rpc(&f, Command::Move { point }, revision)
            .await
            .unwrap_err()
            .code,
        "stale_observation"
    );
    let events = receiver.events();
    let key_events: Vec<_> = events
        .iter()
        .skip(before)
        .filter(|e| (e["type"] == 10 || e["type"] == 11) && e["key"] == 0)
        .collect();
    assert!(key_events.len() >= 4);
    assert_eq!(key_events[0]["type"], 10);
    assert_eq!(key_events[1]["type"], 11); // agent up precedes the viewer down
    assert_eq!(key_events[2]["type"], 10);
    assert_eq!(key_events[3]["type"], 11);
    assert_eq!(
        key_events[2]["flags"].as_u64().unwrap() & ((1 << 17) | (1 << 20)),
        0
    );
    let clicks: Vec<_> = events
        .iter()
        .filter(|e| e["type"] == 1)
        .filter_map(|e| e["clicks"].as_u64())
        .collect();
    assert!(clicks.starts_with(&[1, 2]), "{clicks:?}");
    assert!(
        events
            .iter()
            .any(|e| e["type"] == 3 && e["flags"].as_u64().unwrap() & (1 << 17) != 0)
    );
    assert!(events.iter().any(|e| e["type"] == 6));
    assert!(
        events
            .iter()
            .any(|e| e["type"] == 22 && e["dy"].as_f64().unwrap() < 0.)
    );
    assert!(events.iter().any(|e| e["type"] == 22 && e["phase"] == 8));
    assert!(events.iter().skip(before).any(|e|e["type"]==12&&e["flags"].as_u64().unwrap()&((1<<17)|(1<<20))==0));
    eprintln!(
        "Native RVP acceptance: Unicode text, fresh PNG coordinates, double/right click, drag, scroll, takeover release and stale coordinates passed"
    );
}
