//! Read-only native diagnostics; emits counts and capability flags, never screen/tree contents.
use removent_proto::automation::{Command, Request};
use tokio_util::sync::CancellationToken;
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cancel = CancellationToken::new();
    let _stop = cancel.clone().drop_guard();
    let executor = removent_automation::Executor::native(cancel.clone())?;
    let run = |id: &str, command| {
        executor.execute(
            Request {
                control_revision: None,
                version: 1,
                operation_id: id.into(),
                command,
            },
            cancel.child_token(),
        )
    };
    let capabilities = run("capabilities", Command::Capabilities {}).await?;
    let state = run("state", Command::State {}).await?;
    let mut report = serde_json::json!({"capabilities":capabilities,"app_count":state["apps"].as_array().map(Vec::len),"display_count":state["displays"].as_array().map(Vec::len)});
    if capabilities["screen"] == true {
        match run(
            "screenshot",
            Command::Screenshot {
                display_id: 0,
                max_width: 640,
            },
        )
        .await
        {
            Ok(image) => {
                report["screenshot"] = serde_json::json!({"width":image["width"],"height":image["height"],"observation":image["observation"],"png_base64_bytes":image["data"].as_str().map(str::len)})
            }
            Err(error) => report["screenshot_error"] = serde_json::json!(error.code),
        }
    }
    if capabilities["native_ui"] == true {
        match run(
            "snapshot",
            Command::Snapshot {
                target: "frontmost".into(),
            },
        )
        .await
        {
            Ok(tree) => {
                report["snapshot"] = serde_json::json!({"nodes":tree["nodes"].as_array().map(Vec::len),"truncated":tree["truncated"]})
            }
            Err(error) => report["snapshot_error"] = serde_json::json!(error.code),
        }
    }
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
