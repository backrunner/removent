//! Isolated daemon process lifecycle and real OS IPC test.
use removent_core::{
    DataPaths, Settings,
    ipc::{self, IpcRequest, IpcResponse},
};
use std::time::Duration;
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let paths = DataPaths {
        root: root.path().into(),
    };
    paths.ensure_layout()?;
    let mut settings = Settings::default();
    settings.host_enabled = false;
    settings.prevent_idle_sleep = false;
    settings.save(&paths)?;
    let binary = std::env::current_exe()?
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(format!("removentd{}", std::env::consts::EXE_SUFFIX));
    let mut child = std::process::Command::new(binary)
        .env("REMOVENT_DATA_DIR", &paths.root)
        .spawn()?;
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        let (mut r, mut w) = loop {
            match ipc::connect(&paths).await {
                Ok(pair) => break pair,
                Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        };
        for request in [
            IpcRequest::Status,
            IpcRequest::PairingGenerate,
            IpcRequest::SetEnabled { on: false },
            IpcRequest::Shutdown,
        ] {
            ipc::write_msg(&mut w, &request).await?;
            loop {
                let value: serde_json::Value = ipc::read_msg(&mut r)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("Unexpected IPC EOF"))?;
                if let Ok(response) = serde_json::from_value::<IpcResponse>(value) {
                    match (&request, response) {
                        (IpcRequest::Status, IpcResponse::Status(status)) => {
                            anyhow::ensure!(!status.running, "Unexpected initial sharing state")
                        }
                        (IpcRequest::PairingGenerate, IpcResponse::PairingCode { code, .. }) => {
                            anyhow::ensure!(code.len() == 12, "Invalid pairing invitation")
                        }
                        (IpcRequest::SetEnabled { .. } | IpcRequest::Shutdown, IpcResponse::Ok) => {
                        }
                        (_, other) => anyhow::bail!("Unexpected response {other:?}"),
                    }
                    break;
                }
            }
        }
        Ok::<(), anyhow::Error>(())
    })
    .await;
    // Always reap the isolated subprocess, including failed IPC tests.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while child.try_wait()?.is_none() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    if child.try_wait()?.is_none() {
        let _ = child.kill();
        let _ = child.wait();
        anyhow::bail!("Daemon failed to stop");
    }
    result??;
    anyhow::ensure!(
        removent_core::service_intent::is_stopped(&paths)?,
        "Stop intent was not persisted"
    );
    println!(
        "PASS: daemon status, pairing invitation, sharing toggle, shutdown and stop intent over OS IPC"
    );
    Ok(())
}
