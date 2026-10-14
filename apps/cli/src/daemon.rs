use super::*;
use crate::output::{Failure, command};

type Reader = tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>;
type Writer = tokio::net::unix::OwnedWriteHalf;

fn start_hint() -> String {
    #[cfg(target_os = "macos")]
    return t!("daemon.start_hint", command = command("daemon start")).into();
    #[cfg(not(target_os = "macos"))]
    t!("daemon.start_foreground_hint").into()
}

fn inspect_hint() -> String {
    #[cfg(target_os = "macos")]
    return t!(
        "daemon.inspect_hint",
        command = command("daemon service-status")
    )
    .into();
    #[cfg(not(target_os = "macos"))]
    start_hint()
}

fn failure(key: &str) -> Failure {
    Failure::runtime(t!(key), inspect_hint())
}

async fn connect(paths: &DataPaths) -> anyhow::Result<(Reader, Writer)> {
    removent_core::ipc::connect(paths).await.map_err(|error| {
        let message = match error.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
                Failure::runtime(t!("daemon.unavailable"), start_hint())
            }
            std::io::ErrorKind::PermissionDenied => Failure::runtime(
                t!("daemon.permission_denied"),
                t!("daemon.permission_hint", path = paths.root.display()),
            ),
            _ => failure("daemon.connect_failed"),
        };
        anyhow::Error::new(error).context(message)
    })
}

async fn open_request(
    paths: &DataPaths,
    request: &removent_core::ipc::IpcRequest,
) -> anyhow::Result<(Reader, Writer)> {
    let (reader, mut writer) = connect(paths).await?;
    removent_core::ipc::write_msg(&mut writer, request)
        .await
        .with_context(|| failure("daemon.send_failed"))?;
    Ok((reader, writer))
}

async fn read_response(reader: &mut Reader) -> anyhow::Result<serde_json::Value> {
    let value: serde_json::Value = removent_core::ipc::read_msg(reader)
        .await
        .with_context(|| failure("daemon.read_failed"))?
        .ok_or_else(|| failure("daemon.disconnected"))?;
    Ok(value)
}

fn check_response(value: &serde_json::Value) -> anyhow::Result<()> {
    if value.get("type").and_then(serde_json::Value::as_str) == Some("error") {
        return Err(Failure::runtime(
            value
                .get("message")
                .and_then(serde_json::Value::as_str)
                .filter(|message| !message.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| t!("daemon.request_failed").into()),
            inspect_hint(),
        )
        .into());
    }
    Ok(())
}

/// daemon management subcommand: send a request over UDS and print the response JSON.
pub(super) async fn cmd_daemon(action: &str) -> anyhow::Result<()> {
    use removent_core::ipc::IpcRequest;

    let paths = DataPaths::resolve();
    #[cfg(target_os = "macos")]
    if matches!(
        action,
        "start" | "ensure" | "stop" | "restart" | "login-on" | "login-off" | "service-status"
    ) {
        let service_action = action.to_owned();
        // launchctl and the startup readiness wait are blocking operations.
        let status = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            // Global command aliases point into the bundle. Resolve them before
            // locating the sibling host, including platforms that retain argv[0].
            let exe = std::env::current_exe()?
                .canonicalize()?
                .with_file_name("removentd");
            let service = removent_core::service::Service::new(paths, exe)?;
            match service_action.as_str() {
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
        .await
        .context("Service task failed")?
        .with_context(|| {
            Failure::runtime(
                t!("daemon.service_failed", action = action),
                t!(
                    "daemon.service_hint",
                    path = DataPaths::resolve()
                        .logs_dir()
                        .join("removentd.log")
                        .display()
                ),
            )
        })?;
        println!("{}", serde_json::to_string_pretty(&status)?);
        return Ok(());
    }

    let req = match action {
        "status" => IpcRequest::Status,
        "reload" => IpcRequest::ReloadSettings,
        "pairing-show" => IpcRequest::PairingShow,
        "pairing-generate" => IpcRequest::PairingGenerate,
        "pairing-revoke" => IpcRequest::PairingRevoke,
        "permissions" => IpcRequest::RequestPermissions,
        "enable" => IpcRequest::SetEnabled { on: true },
        "disable" => IpcRequest::SetEnabled { on: false },
        other => bail!(t!("daemon.unknown_action", action = other)),
    };

    let resp = request(&paths, &req).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&resp).context(t!("daemon.serialize_failed"))?
    );
    Ok(())
}

pub(super) async fn request(
    paths: &DataPaths,
    req: &removent_core::ipc::IpcRequest,
) -> anyhow::Result<serde_json::Value> {
    // The event stream may interleave with responses; take the first response (with a timeout).
    let resp: serde_json::Value = tokio::time::timeout(Duration::from_secs(5), async {
        let (mut r, _w) = open_request(paths, req).await?;
        loop {
            let v = read_response(&mut r).await?;
            check_response(&v)?;
            match v.get("type").and_then(serde_json::Value::as_str) {
                Some("status" | "ok" | "error" | "pairing_code") => {
                    return Ok::<_, anyhow::Error>(v);
                }
                _ => continue,
            }
        }
    })
    .await
    .with_context(|| failure("daemon.wait_timeout"))??;
    Ok(resp)
}

/// Stream pairing codes from the local private IPC socket for headless hosts.
pub(super) async fn watch_pairing() -> anyhow::Result<()> {
    tokio::select! {
        result = watch_pairing_inner() => result,
        result = tokio::signal::ctrl_c() => result.map_err(Into::into),
    }
}

async fn watch_pairing_inner() -> anyhow::Result<()> {
    use removent_core::ipc::IpcRequest;
    let (mut r, _w) = tokio::time::timeout(
        Duration::from_secs(5),
        open_request(&DataPaths::resolve(), &IpcRequest::PairingShow),
    )
    .await
    .with_context(|| failure("daemon.wait_timeout"))??;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut received_initial = false;
    loop {
        let value = if received_initial {
            read_response(&mut r).await?
        } else {
            tokio::time::timeout_at(deadline, read_response(&mut r))
                .await
                .with_context(|| failure("daemon.wait_timeout"))??
        };
        // PairingShow responds with Error when no code is active. A watcher
        // must keep listening for future codes in that normal idle state.
        if value.get("type").and_then(serde_json::Value::as_str) == Some("error") {
            received_initial = true;
            continue;
        }
        if let Some("pairing_code" | "pairing_pin" | "pairing_done" | "pairing_cleared") =
            value.get("type").and_then(serde_json::Value::as_str)
        {
            received_initial = true;
            println!("{}", serde_json::to_string(&value)?);
        }
    }
}
