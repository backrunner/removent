//! Opt-in full MCP -> RVP -> native AppKit acceptance. No user data is modified.
use super::*;
use std::{
    path::PathBuf,
    process::{Child, Command as Process},
    time::Instant,
};
type Output = tokio::io::Lines<BufReader<tokio::process::ChildStdout>>;

struct Receiver {
    child: Child,
    directory: tempfile::TempDir,
    target: String,
}
impl Drop for Receiver {
    fn drop(&mut self) {
        let _ = std::fs::write(self.directory.path().join("quit"), []);
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Receiver {
    async fn start() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("receiver");
        let built = Process::new("xcrun")
            .arg("swiftc")
            .arg(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../scripts/qa/automation_receiver.swift"),
            )
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        assert!(
            built.status.success(),
            "{}",
            String::from_utf8_lossy(&built.stderr)
        );
        let child = Process::new(executable)
            .arg(directory.path())
            .spawn()
            .unwrap();
        let mut receiver = Self {
            child,
            directory,
            target: String::new(),
        };
        tokio::time::timeout(Duration::from_secs(6), async {
            while !receiver.directory.path().join("ready.json").exists() {
                assert!(
                    receiver.child.try_wait().unwrap().is_none(),
                    "Fixture exited; unlock the desktop before running native acceptance"
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        let ready: Value = serde_json::from_slice(
            &std::fs::read(receiver.directory.path().join("ready.json")).unwrap(),
        )
        .unwrap();
        receiver.target = format!("app:{}", ready["pid"].as_u64().unwrap());
        receiver
    }
    async fn expect_ui(&self, key: &str, expected: Value) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let ui = std::fs::read(self.directory.path().join("ui.json"))
                    .ok()
                    .and_then(|data| serde_json::from_slice::<Value>(&data).ok());
                if ui.is_some_and(|value| value[key] == expected) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
    }
}
async fn computer(
    input: &mut tokio::process::ChildStdin,
    output: &mut Output,
    session: &str,
    id: u64,
    operation: &str,
    command: Value,
) -> Value {
    let response = call(
        input,
        output,
        id,
        "computer",
        json!({"session_id":session,"operation_id":operation,"command":command}),
    )
    .await;
    assert_ne!(
        response["isError"], true,
        "{}",
        response["structuredContent"]
    );
    response
}
async fn foreground(
    input: &mut tokio::process::ChildStdin,
    output: &mut Output,
    session: &str,
    receiver: &Receiver,
    id: u64,
) {
    let state = computer(
        input,
        output,
        session,
        id,
        &format!("state-{id}"),
        json!({"action":"state"}),
    )
    .await;
    assert!(
        state["structuredContent"]["apps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|app| app["target"] == receiver.target && app["active"] == true),
        "Fixture lost foreground; refusing to inject input"
    );
}
pub async fn exercise(input: &mut tokio::process::ChildStdin, output: &mut Output, session: &str) {
    let receiver = Receiver::start().await;
    for id in 200..300 {
        let control = computer(
            input,
            output,
            session,
            id,
            &format!("control-{id}"),
            json!({"action":"control_status"}),
        )
        .await;
        let control = &control["structuredContent"];
        assert_ne!(
            control["local_input_monitor"]["state"], "unavailable",
            "{control}"
        );
        if control["local_input_monitor"]["state"] == "ready" {
            assert_eq!(control["mode"], "automation", "Human input paused the test");
            break;
        }
        assert!(id < 299, "Monitor did not start");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    foreground(input, output, session, &receiver, 300).await;
    let tree = computer(
        input,
        output,
        session,
        301,
        "tree",
        json!({"action":"snapshot","target":receiver.target}),
    )
    .await;
    let nodes = tree["structuredContent"]["nodes"].as_array().unwrap();
    let field = nodes
        .iter()
        .find(|n| n["identifier"] == "automation-text")
        .unwrap()["reference"]
        .clone();
    let button = nodes
        .iter()
        .find(|n| n["identifier"] == "automation-button")
        .unwrap()["reference"]
        .clone();
    foreground(input, output, session, &receiver, 302).await;
    computer(
        input,
        output,
        session,
        303,
        "value",
        json!({"action":"set_value","reference":field,"value":"base"}),
    )
    .await;
    computer(
        input,
        output,
        session,
        304,
        "selection",
        json!({"action":"select_text","reference":field,"text":"base","placement":"select"}),
    )
    .await;
    for id in 305..307 {
        foreground(input, output, session, &receiver, id + 100).await;
        computer(
            input,
            output,
            session,
            id,
            "type-once",
            json!({"action":"type_text","text":"MCP 中文😀"}),
        )
        .await;
    }
    receiver.expect_ui("text", json!("MCP 中文😀")).await;
    for id in 307..309 {
        computer(
            input,
            output,
            session,
            id,
            "press-once",
            json!({"action":"element_action","reference":button,"name":"AXPress"}),
        )
        .await;
    }
    receiver.expect_ui("count", json!(1)).await;
    let image = computer(
        input,
        output,
        session,
        309,
        "screenshot",
        json!({"action":"screenshot","display_id":0,"max_width":640}),
    )
    .await;
    assert!(image["structuredContent"].get("data").is_none());
    assert_eq!(image["content"][1]["type"], "image");
    assert!(
        image["content"][1]["data"]
            .as_str()
            .unwrap()
            .starts_with("iVBORw0KGgo")
    );
    assert_eq!(image["structuredContent"]["width"], 640);
    eprintln!(
        "Native MCP acceptance: stdio connect -> pinned RVP -> AppKit value/selection/Unicode/button, mutation retry dedup and real PNG image content passed"
    );
}
