use super::*;

/// daemon management subcommand: send a request over UDS and print the response JSON.
pub(super) async fn cmd_daemon(action: &str) -> anyhow::Result<()> {
    use removent_core::ipc::{IpcRequest, read_msg, write_msg};

    let paths = DataPaths::resolve();
    #[cfg(target_os = "macos")]
    if matches!(
        action,
        "start" | "ensure" | "stop" | "restart" | "login-on" | "login-off" | "service-status"
    ) {
        let exe = std::env::current_exe()?.with_file_name("removentd");
        let service = removent_core::service::Service::new(paths, exe)?;
        let action = action.to_owned();
        // launchctl and the startup readiness wait are blocking operations.
        let status = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            match action.as_str() {
                "start" => service.start()?,
                "ensure" => service.ensure_running()?,
                "stop" => service.stop()?,
                "restart" => service.restart()?,
                "login-on" => service.set_launch_at_login(true)?,
                "login-off" => service.set_launch_at_login(false)?,
                _ => {}
            }
            service.status()
        })
        .await??;
        println!("{}", serde_json::to_string_pretty(&status)?);
        return Ok(());
    }

    let req = match action {
        "status" => IpcRequest::Status,
        "permissions" => IpcRequest::RequestPermissions,
        "enable" => IpcRequest::SetEnabled { on: true },
        "disable" => IpcRequest::SetEnabled { on: false },
        other => bail!(t!("daemon.unknown_action", action = other)),
    };

    let (mut r, mut w) = removent_core::ipc::connect(&paths)
        .await
        .context(t!("daemon.connect_failed"))?;
    write_msg(&mut w, &req)
        .await
        .context(t!("daemon.send_failed"))?;

    // The event stream may interleave with responses; take the first response (with a timeout).
    let resp: serde_json::Value = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let v: serde_json::Value = read_msg(&mut r)
                .await
                .context(t!("daemon.read_failed"))?
                .context(t!("daemon.disconnected"))?;
            match v.get("type").and_then(serde_json::Value::as_str) {
                Some("status" | "ok" | "error") => return Ok::<_, anyhow::Error>(v),
                _ => continue,
            }
        }
    })
    .await
    .context(t!("daemon.wait_timeout"))??;
    if resp.get("type").and_then(serde_json::Value::as_str) == Some("error") {
        bail!(
            "{}",
            resp.get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Daemon request failed")
        );
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&resp).context(t!("daemon.serialize_failed"))?
    );
    Ok(())
}
