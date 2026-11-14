//! Isolated tray QA access to the real management protocol, never a fake daemon.
use removent_core::ipc::{self, IpcRequest, IpcResponse};
use std::io::Write;
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    anyhow::ensure!(
        std::env::var("REMOVENT_DESKTOP_TEST").as_deref() == Ok("1"),
        "Requires isolated desktop"
    );
    let args: Vec<_> = std::env::args().collect();
    let paths = removent_core::DataPaths::resolve();
    tokio::time::timeout(std::time::Duration::from_secs(1200), async {
        let (mut r, mut w) = ipc::connect(&paths).await?;
        let watch = args.get(1).is_some_and(|s| s == "watch");
        let request: IpcRequest = if watch {
            IpcRequest::Status
        } else {
            serde_json::from_str(
                args.get(1)
                    .ok_or_else(|| anyhow::anyhow!("Expected request JSON or watch"))?,
            )?
        };
        ipc::write_msg(&mut w, &request).await?;
        while let Some(value) = ipc::read_msg::<_, serde_json::Value>(&mut r).await? {
            let response = serde_json::from_value::<IpcResponse>(value.clone()).is_ok();
            if watch || response {
                println!("{value}");
                std::io::stdout().flush()?;
                if !watch {
                    return Ok::<_, anyhow::Error>(());
                }
            }
        }
        anyhow::ensure!(watch, "IPC closed without a response");
        Ok(())
    })
    .await??;
    Ok(())
}
