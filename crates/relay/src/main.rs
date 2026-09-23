mod cli;
mod management;
mod updater;

use anyhow::{Context, Result, ensure};
use tokio_util::sync::CancellationToken;

fn main() -> Result<()> {
    let mut runtime = tokio::runtime::Builder::new_multi_thread();
    // Forwarding is asynchronous I/O; do not allocate a worker per core on a
    // large VPS by default. Tokio's explicit deployment override still works.
    if std::env::var_os("TOKIO_WORKER_THREADS").is_none() {
        runtime.worker_threads(std::thread::available_parallelism().map_or(1, |n| n.get().min(2)));
    }
    let next = runtime.enable_all().build()?.block_on(run())?;
    if let Some(binary) = next {
        use std::os::unix::process::CommandExt;
        let error = std::process::Command::new(&binary)
            .args(std::env::args_os().skip(1))
            .exec();
        // If exec fails, don't repeatedly select an unusable cached release on
        // the next supervisor restart. The installed launcher is still intact.
        updater::rollback(&binary)?;
        return Err(error).context("Cannot launch updated relay; restored previous selection");
    }
    Ok(())
}

async fn run() -> Result<Option<std::path::PathBuf>> {
    let Some((config, websocket, container)) = cli::execute().await? else {
        return Ok(None);
    };
    config.validate()?;
    if !container {
        match updater::installed(&config) {
            Ok(Some(binary)) => return Ok(Some(binary)),
            Ok(None) => {}
            Err(error) => eprintln!("Removent: ignoring unusable cached relay update: {error}"),
        }
    }
    let paths = removent_core::DataPaths {
        root: config.identity_dir.clone(),
    };
    removent_core::logging::init_component_logging(
        &paths,
        "removent-relay",
        std::env::var_os("REMOVENT_RELAY_READY_FILE").is_none(),
    );
    removent_core::logging::install_panic_hook(&paths);
    // The process lock covers both transports, every port and config alias
    // sharing this identity. Acquire it before identity creation or listening.
    let _instance =
        removent_core::DataDirLock::acquire_at(&config.identity_dir.join(".relay.lock"))
            .context("A relay is already running for this identity, or its lock is unavailable")?;
    let stop = CancellationToken::new();
    let mut update_config = config.clone();
    // Container images are rolled out through Cloudflare, never self-updated.
    if container {
        update_config.updates.enabled = false;
    }
    let update = updater::schedule(update_config);
    tokio::pin!(update);
    let mut task = if websocket {
        let idle: u64 = std::env::var("REMOVENT_RELAY_IDLE_SECS")
            .unwrap_or_else(|_| "300".into())
            .parse()
            .context("Invalid relay idle timeout")?;
        ensure!(
            idle == 0 || (60..=86400).contains(&idle),
            "Idle timeout must be 0 (disabled) or 60..86400 seconds"
        );
        let listener = tokio::net::TcpListener::bind(config.listen).await?;
        tracing::info!(address = %listener.local_addr()?, idle_seconds = idle, "WebSocket relay listening (TLS required at ingress)");
        tokio::spawn(removent_relay::websocket::serve(
            listener,
            config,
            stop.clone(),
            (idle != 0).then(|| std::time::Duration::from_secs(idle)),
        ))
    } else {
        let identity = removent_core::identity::load_or_create(
            &removent_core::DataPaths {
                root: config.identity_dir.clone(),
            },
            "Removent relay",
        )?;
        let endpoint = removent_relay::server_endpoint(config.listen, &identity)?;
        tracing::info!(address = %endpoint.local_addr()?, fingerprint = %identity.fingerprint_hex(), "Relay listening");
        tokio::spawn(removent_relay::server::serve(
            endpoint,
            config,
            stop.clone(),
        ))
    };
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    // Tell systemd we are ready only after the transport has successfully bound.
    let _ready = management::notify_ready()?;
    let next = tokio::select! {
        biased;
        _ = tokio::signal::ctrl_c() => None,
        _ = term.recv() => None,
        result = &mut task => { result.context("Relay task failed")??; return Ok(None); },
        binary = &mut update => {
            tracing::info!("Restarting relay to apply verified update");
            Some(binary)
        },
    };
    stop.cancel();
    task.await??;
    Ok(next)
}
