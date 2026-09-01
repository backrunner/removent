//! removentd entry point: data-dir lock + tokio runtime + IPC server + host manager + signal shutdown.

use anyhow::Context;
use removent_core::{DataDirLock, DataPaths, Settings, identity, logging};
use removent_daemon::{hostmgr, server, state::DaemonState};
use rust_i18n::t;
use std::sync::Arc;

rust_i18n::i18n!("locales");

fn main() -> anyhow::Result<()> {
    let paths = DataPaths::resolve();
    paths.ensure_layout().context(t!("error.init_data_dir"))?;
    logging::init_logging(&paths);
    logging::install_panic_hook(&paths);

    let settings = Settings::load(&paths).unwrap_or_default();
    rust_i18n::set_locale(removent_core::resolve_locale(settings.language));

    ensure_host_permissions(settings.host_enabled);

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
    let settings = Settings::load(&paths).unwrap_or_default();
    let identity = identity::load_or_create(&paths, &settings.device_name)
        .context(t!("error.load_identity"))?;
    let state = Arc::new(DaemonState::new(
        paths,
        settings,
        identity.short_fingerprint_hex(),
    ));
    tracing::info!(fp=%state.fp_short, enabled=%state.enabled.load(std::sync::atomic::Ordering::SeqCst), "removentd started");

    let ipc = tokio::spawn(server::serve(state.clone()));
    let mgr = tokio::spawn(hostmgr::run(state.clone()));

    // SIGTERM / SIGINT / IPC Shutdown all trigger graceful shutdown.
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context(t!("error.register_sigterm"))?;
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {},
        _ = sigterm.recv() => {},
        _ = state.shutdown.cancelled() => {},
    }
    tracing::info!("removentd shutting down");
    state.shutdown.cancel();
    let _ = ipc.await;
    let _ = mgr.await;
    Ok(())
}

/// Screen Recording + Accessibility TCC checks for the hosting path.
///
/// When the app spawns the daemon, the app is the TCC responsible process and its grant
/// covers us. But a launchd-started daemon (LaunchAgent, launch at login) is its own
/// responsible process with its own TCC identity, so the app's grant does NOT apply —
/// preflight here and trigger the consent prompts (attributed to `removentd`) when the
/// host service is enabled. Without this, capture and input injection silently fail
/// for headless autostart.
#[cfg(target_os = "macos")]
fn ensure_host_permissions(host_enabled: bool) {
    use removent_daemon::tcc;
    if !host_enabled {
        return;
    }
    if !tcc::screen_recording_granted() {
        tracing::warn!("screen recording not granted to removentd; requesting");
        let granted = tcc::request_screen_recording();
        if !granted {
            tracing::warn!(
                "screen recording still not granted; capture will fail until removentd is allowed under System Settings > Privacy & Security > Screen Recording (the daemon must restart afterwards)"
            );
        }
    }
    if !tcc::accessibility_granted() {
        tracing::warn!("accessibility not granted to removentd; requesting");
        let granted = tcc::request_accessibility();
        if !granted {
            tracing::warn!(
                "accessibility still not granted; input injection will fail until removentd is allowed under System Settings > Privacy & Security > Accessibility"
            );
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn ensure_host_permissions(_host_enabled: bool) {}
