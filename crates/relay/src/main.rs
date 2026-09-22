mod cli;
mod management;
mod remote;

use anyhow::{Context, Result, ensure};
use tokio_util::sync::CancellationToken;

fn main() -> Result<()> {
    let mut runtime = tokio::runtime::Builder::new_multi_thread();
    // Forwarding is asynchronous I/O; do not allocate a worker per core on a
    // large VPS by default. Tokio's explicit deployment override still works.
    if std::env::var_os("TOKIO_WORKER_THREADS").is_none() {
        runtime.worker_threads(std::thread::available_parallelism().map_or(1, |n| n.get().min(2)));
    }
    runtime.enable_all().build()?.block_on(run())
}

async fn run() -> Result<()> {
    let Some((config, websocket)) = cli::execute().await? else {
        return Ok(());
    };
    tracing_subscriber::fmt().with_env_filter("info").init();
    config.validate()?;
    let stop = CancellationToken::new();
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
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {},
        _ = term.recv() => {},
        result = &mut task => { result.context("Relay task failed")??; return Ok(()); },
    }
    stop.cancel();
    task.await??;
    Ok(())
}
