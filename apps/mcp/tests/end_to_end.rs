//! Real stdio MCP -> pinned RVP session -> deterministic host executor.
use removent_automation::{Executor, Outcome, Provider};
use removent_core::{AdmissionMode, DataPaths, PeerRecord, PeersStore, identity};
use removent_net::{PinState, RvpConnection, make_server_endpoint};
use removent_proto::{Caps, automation::Command};
use serde_json::{Value, json};
use std::{
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;
struct Fake(Arc<AtomicUsize>);
impl Provider for Fake {
    fn execute(&mut self, command: &Command, _: &CancellationToken) -> Outcome {
        match command {
            Command::Capabilities {} => {
                Ok(json!({"screen":true,"native_ui":true,"browser":{"ready":false}}))
            }
            Command::Screenshot { .. } => {
                Ok(json!({"data":"AA==","width":1,"height":1,"observation":1}))
            }
            _ => Ok(json!({"executed":self.0.fetch_add(1,Ordering::SeqCst)+1})),
        }
    }
}
async fn rpc(
    input: &mut tokio::process::ChildStdin,
    output: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    id: u64,
    method: &str,
    params: Value,
) -> Value {
    input
        .write_all(
            format!(
                "{}\n",
                json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    loop {
        let line = tokio::time::timeout(Duration::from_secs(10), output.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let message: Value = serde_json::from_str(&line).unwrap();
        if message["id"] == id {
            return message["result"].clone();
        }
    }
}
async fn call(
    input: &mut tokio::process::ChildStdin,
    output: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    id: u64,
    name: &str,
    args: Value,
) -> Value {
    rpc(
        input,
        output,
        id,
        "tools/call",
        json!({"name":name,"arguments":args}),
    )
    .await
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcp_initiates_connection_returns_remote_ack_and_shuts_down() {
    exercise(false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires an unlocked macOS GUI, Accessibility, Input Monitoring and Screen Recording permissions"]
async fn mcp_native_appkit_end_to_end() {
    exercise(true).await;
}
#[path = "end_to_end/native.rs"]
mod native;
async fn exercise(native: bool) {
    let dir = tempfile::tempdir().unwrap();
    let paths = DataPaths {
        root: dir.path().into(),
    };
    paths.ensure_layout().unwrap();
    let client = identity::load_or_create(&paths, "MCP").unwrap();
    let hd = tempfile::tempdir().unwrap();
    let host = identity::load_or_create(
        &DataPaths {
            root: hd.path().into(),
        },
        "Host",
    )
    .unwrap();
    let (server, _) = make_server_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &host,
        PinState::new([client.fingerprint], false),
    )
    .unwrap();
    let address = server.local_addr().unwrap();
    removent_client::host_pins::remember(&paths, &address.to_string(), host.fingerprint, false)
        .unwrap();
    let saved = removent_client::saved::SavedConnections::load(&paths)
        .unwrap()
        .upsert(removent_client::saved::SavedConnection {
            host: address.ip().to_string(),
            port: address.port(),
            name: "Fixture".into(),
            ..Default::default()
        })
        .unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let fake = Fake(count.clone());
    let (executor_tx, executor_rx) = tokio::sync::oneshot::channel();
    let host_task = tokio::spawn(async move {
        let mut peers = PeersStore::in_memory();
        peers
            .upsert(PeerRecord {
                fingerprint: client.fingerprint_hex(),
                short_fp: client.short_fingerprint_hex(),
                name: "MCP".into(),
                granted_caps: Caps::all(),
                trusted: true,
                added_at_unix: 0,
                last_connected_unix: 0,
            })
            .unwrap();
        let conn = RvpConnection::new(server.accept().await.unwrap().await.unwrap());
        let cfg = removent_host::HostConfig {
            authentication: removent_core::AuthenticationSettings {
                mode: removent_core::AuthenticationMode::PairingCode,
                ..Default::default()
            },
            auth_paths: None,
            audio_available: false,
            preapproved_only: false,
            device_name: "Fixture".into(),
            admission: AdmissionMode::TrustedAuto,
            video_bitrate_kbps: 3000,
            video_fps: 30,
            input_sink: Some(Arc::new(removent_host::RecorderInputSink::default())),
            local_clip: None,
        };
        let interactions = removent_host::HostInteractions {
            show_pairing_pin: Box::new(|_| panic!("Already paired")),
            admission_prompt: Box::new(|_, _| Box::pin(async { panic!("Already trusted") })),
        };
        let (s, _, _, commands, sink, source) = removent_host::serve_connection(
            conn.clone(),
            &host,
            &mut peers,
            &cfg,
            interactions,
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
        let executor = if native {
            Executor::native(s.cancel.clone()).unwrap()
        } else {
            Executor::spawn(s.cancel.clone(), move || (fake, ())).unwrap()
        };
        executor_tx.send(executor.clone()).ok().unwrap();
        removent_host::spawn_control_pump(
            source,
            sink,
            removent_host::ControlPumpDeps {
                automation: Some(executor),
                conn,
                kf_tx: None,
                controller: None,
                window_ms: 250,
                input: cfg.input_sink,
                local_clip: None,
                quality_tx: None,
                caps: s.peer_caps,
                clip_state: None,
                cancel: s.cancel,
                peer_fp: None,
                delivery: None,
            },
            commands,
        )
        .await
        .unwrap();
    });
    let mut process = tokio::process::Command::new(env!("CARGO_BIN_EXE_removent-mcp"))
        .env("REMOVENT_DATA_DIR", dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = process.stdin.take().unwrap();
    let mut output = BufReader::new(process.stdout.take().unwrap()).lines();
    assert_eq!(rpc(&mut input,&mut output,1,"initialize",json!({"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}})).await["serverInfo"]["name"],"removent-mcp");
    input
        .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
        .await
        .unwrap();
    let started = call(
        &mut input,
        &mut output,
        2,
        "removent_connect",
        json!({"saved_id":saved.id}),
    )
    .await;
    let id = started["structuredContent"]["session_id"].as_str().unwrap();
    let mut ready = false;
    for n in 3..100 {
        let status = call(
            &mut input,
            &mut output,
            n,
            "removent_status",
            json!({"session_id":id}),
        )
        .await;
        if status["structuredContent"]["status"] == "connected" {
            ready = true;
            break;
        }
        assert_ne!(status["structuredContent"]["status"], "failed", "{status}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(ready);
    if native {
        native::exercise(&mut input, &mut output, id).await;
    } else {
        let args = json!({"session_id":id,"operation_id":"type_once","command":{"action":"type_text","text":"test"}});
        for n in 101..103 {
            let result = call(&mut input, &mut output, n, "computer", args.clone()).await;
            assert_eq!(result["structuredContent"]["executed"], 1, "{result}");
        }
        let image=call(&mut input,&mut output,103,"computer",json!({"session_id":id,"operation_id":"screen","command":{"action":"screenshot","display_id":0,"max_width":640}})).await;
        assert!(image["structuredContent"].get("data").is_none());
        assert_eq!(image["content"][1]["type"], "image");
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let executor = executor_rx.await.unwrap();
        drop(executor.manual_input(false).await.unwrap());
        let paused = call(
            &mut input,
            &mut output,
            104,
            "removent_status",
            json!({"session_id":id}),
        )
        .await;
        assert_eq!(paused["structuredContent"]["control"]["mode"], "manual");
        let blocked=call(&mut input,&mut output,105,"computer",json!({"session_id":id,"operation_id":"blocked","command":{"action":"type_text","text":"no"}})).await;
        assert_eq!(
            blocked["structuredContent"]["error"]["code"],
            "control_changed"
        );
        tokio::time::sleep(Duration::from_millis(510)).await;
        let resumed=call(&mut input,&mut output,106,"computer",json!({"session_id":id,"operation_id":"resume","command":{"action":"resume_control","revision":paused["structuredContent"]["control"]["revision"]}})).await;
        assert_eq!(resumed["structuredContent"]["mode"], "automation");
        let fresh=call(&mut input,&mut output,107,"computer",json!({"session_id":id,"operation_id":"fresh","command":{"action":"type_text","text":"yes"}})).await;
        assert_eq!(fresh["structuredContent"]["executed"], 2);
        // The platform listener's host-side notification uses the same MCP state
        // and resume contract. No OS event is injected by this protocol test.
        executor.local_input(true);
        let local = call(
            &mut input,
            &mut output,
            108,
            "removent_status",
            json!({"session_id":id}),
        )
        .await;
        assert_eq!(
            local["structuredContent"]["control"]["last_input_source"],
            "local"
        );
        assert_eq!(
            local["structuredContent"]["control"]["local_input_held"],
            true
        );
        let busy = call(&mut input, &mut output, 109, "computer", json!({"session_id":id,"operation_id":"local-busy","command":{"action":"resume_control","revision":local["structuredContent"]["control"]["revision"]}})).await;
        assert_eq!(busy["structuredContent"]["error"]["code"], "manual_busy");
        executor.local_input(false);
        let released = call(
            &mut input,
            &mut output,
            110,
            "removent_status",
            json!({"session_id":id}),
        )
        .await;
        tokio::time::sleep(Duration::from_millis(510)).await;
        let resumed = call(&mut input, &mut output, 111, "computer", json!({"session_id":id,"operation_id":"local-resume","command":{"action":"resume_control","revision":released["structuredContent"]["control"]["revision"]}})).await;
        assert_eq!(resumed["structuredContent"]["mode"], "automation");
        let fresh = call(&mut input, &mut output, 112, "computer", json!({"session_id":id,"operation_id":"local-fresh","command":{"action":"type_text","text":"after local resume"}})).await;
        assert_eq!(fresh["structuredContent"]["executed"], 3);
    }
    drop(input);
    assert!(
        tokio::time::timeout(Duration::from_secs(3), process.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    tokio::time::timeout(Duration::from_secs(3), host_task)
        .await
        .unwrap()
        .unwrap();
}
