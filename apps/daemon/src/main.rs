//! removentd entry point: data-dir lock + tokio runtime + IPC server + host manager + signal shutdown.

use anyhow::Context;
use removent_core::{DataDirLock, DataPaths, Settings, identity, logging};
use removent_daemon::{hostmgr, server, state::DaemonState};
use rust_i18n::t;
use std::sync::Arc;

rust_i18n::i18n!("locales");

fn main() -> anyhow::Result<()> {
    let login_window = std::env::args().any(|arg| arg == "--login-window");
    let paths = if login_window {
        removent_daemon::login_window::paths()?
    } else {
        DataPaths::resolve()
    };
    logging::init_logging(&paths);
    logging::install_panic_hook(&paths);
    let result = start(paths);
    if let Err(error) = &result {
        tracing::error!(error = %format!("{error:#}"), "daemon exited with an error");
    }
    result
}

fn start(paths: DataPaths) -> anyhow::Result<()> {
    paths.ensure_layout().context(t!("error.init_data_dir"))?;
    if std::env::args().any(|arg| arg == "--background")
        && removent_core::service_intent::is_stopped(&paths)?
    {
        return Ok(());
    }
    let settings = Settings::load(&paths)?;
    rust_i18n::set_locale(removent_core::resolve_locale(settings.language));

    // Single instance: a daemon-specific lock, separate from the app's .lock.
    let _lock = match DataDirLock::acquire_at(&paths.daemon_lock_file()) {
        Ok(lock) => lock,
        Err(_) => {
            eprintln!(
                "{}",
                t!(
                    "error.already_running",
                    path = paths.daemon_lock_file().display().to_string()
                )
            );
            std::process::exit(1);
        }
    };

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context(t!("error.create_runtime"))?;
    rt.block_on(run(paths))
}

async fn run(paths: DataPaths) -> anyhow::Result<()> {
    let mut settings = Settings::load(&paths)?;
    if std::env::args().any(|arg| arg == "--login-window") {
        removent_daemon::login_window::restrict(&mut settings);
    }
    let identity = identity::load_or_create(&paths, &settings.device_name)
        .context(t!("error.load_identity"))?;
    let mut state = DaemonState::new(paths, settings, identity.short_fingerprint_hex());
    state.login_window = std::env::args().any(|arg| arg == "--login-window");
    let state = Arc::new(state);
    tracing::info!(fp=%state.fp_short, enabled=%state.enabled.load(std::sync::atomic::Ordering::SeqCst), "removentd started");

    let mut ipc = tokio::spawn(server::serve(state.clone()));
    let mgr = tokio::spawn(hostmgr::run(state.clone()));

    // Prompt from the actual hosting process, including LaunchAgent startup.
    // A detached worker keeps consent dialogs from delaying IPC or shutdown.
    removent_daemon::tcc::request_initial_permissions(&state);

    // SIGTERM / SIGINT / IPC Shutdown all trigger graceful shutdown.
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context(t!("error.register_sigterm"))?;
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {},
        _ = sigterm.recv() => {},
        _ = state.shutdown.cancelled() => {},
        result = &mut ipc => {
            state.shutdown.cancel();
            let _ = mgr.await;
            result.context("IPC task failed")??;
            anyhow::bail!("IPC server stopped unexpectedly");
        },
    }
    tracing::info!("removentd shutting down");
    state.shutdown.cancel();
    let _ = ipc.await;
    let _ = mgr.await;
    Ok(())
}
