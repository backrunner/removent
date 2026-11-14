use super::*;

/// Hand off a verified update to the independent agent. The UI can then quit.
#[cfg(target_os = "macos")]
pub fn run_install(
    generation: u64,
    shared: Arc<Mutex<UpdateShared>>,
    events: std::sync::mpsc::Sender<UiEvent>,
    paths: DataPaths,
) {
    // Concurrency guard (double-click on "Install"): claim the ReadyToInstall →
    // Swapping transition inside one lock; a second call returns instead of
    // racing swap_bundle (whose backup cleanup would delete the first swap's
    // rollback copy).
    let staged = {
        let mut s = shared.lock().unwrap();
        if s.generation != generation {
            return;
        }
        let UpdateStatus::ReadyToInstall { version } = &s.status else {
            return;
        };
        let Some(staged) = s.staged_app.clone() else {
            return;
        };
        if !eligible_upgrade(s.policy.channel, version, env!("CARGO_PKG_VERSION"))
            || s.manifest.as_ref().is_none_or(|m| m.version != *version)
        {
            s.status = UpdateStatus::Failed(t!("update.err.not_upgrade").to_string());
            let _ = events.send(UiEvent::UpdateStatus(s.status.clone()));
            return;
        }
        let version = version.clone();
        s.status = UpdateStatus::Swapping {
            version: version.clone(),
        };
        let _ = events.send(UiEvent::UpdateStatus(s.status.clone()));
        staged
    };
    let result = (|| -> anyhow::Result<()> {
        let bundle = current_bundle()
            .ok_or_else(|| anyhow::anyhow!(t!("update.err.not_in_bundle").to_string()))?;
        let (manifest, policy) = {
            let state = shared.lock().unwrap();
            (
                state
                    .manifest
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("Update manifest is missing"))?,
                state.policy.clone(),
            )
        };
        crate::agent::request_install(&paths, &bundle, &manifest, &staged, &policy)
    })();
    match result {
        Ok(()) => set_status(&shared, &events, UpdateStatus::Relaunching),
        Err(error) => set_status(&shared, &events, UpdateStatus::Failed(error.to_string())),
    }
}

/// Stage on the destination volume before moving the running app. A failed
/// cross-volume copy must never leave a partial new bundle blocking rollback.
#[cfg(test)]
pub(super) fn replace_bundle(staged_app: &Path, bundle: &Path) -> Result<(), String> {
    let incoming = bundle.with_extension("app.incoming");
    let backup = bundle.with_extension("app.old");
    let fail = |e: std::io::Error| t!("update.err.swap", err = e.to_string()).to_string();
    if incoming.exists() {
        std::fs::remove_dir_all(&incoming).map_err(fail)?;
    }
    if let Err(error) = move_into_place(staged_app, &incoming) {
        let _ = std::fs::remove_dir_all(&incoming);
        return Err(fail(error));
    }
    // Copying has completed. Only the two same-volume renames happen while
    // the original application path is temporarily unavailable.
    if backup.exists() {
        std::fs::remove_dir_all(&backup).map_err(fail)?;
    }
    std::fs::rename(bundle, &backup).map_err(fail)?;
    if let Err(error) = std::fs::rename(&incoming, bundle) {
        if std::fs::rename(&backup, bundle).is_err() {
            return Err(t!(
                "update.err.swap_rollback",
                err = error.to_string(),
                backup = backup.display().to_string()
            )
            .to_string());
        }
        return Err(fail(error));
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn move_into_place(staged: &Path, bundle: &Path) -> std::io::Result<()> {
    match std::fs::rename(staged, bundle) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            let copied = Command::new("/usr/bin/ditto")
                .arg(staged)
                .arg(bundle)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()?;
            if !copied.success() {
                return Err(std::io::Error::other(format!("ditto exit {copied}")));
            }
            // Failure to remove the cache must not invalidate a successful copy.
            let _ = std::fs::remove_dir_all(staged);
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// Service::restart serializes against watchdogs, unloads KeepAlive and waits
/// for the old daemon lock to release before launching the new executable.
#[cfg(all(test, target_os = "macos"))]
pub(super) fn restart_background(
    bundle: &Path,
    paths: &DataPaths,
    resume_service: bool,
) -> std::io::Result<()> {
    let service = removent_core::service::Service::new(
        paths.clone(),
        bundle.join("Contents/MacOS/removentd"),
    )
    .map_err(std::io::Error::other)?;
    if resume_service {
        service.restart().map_err(std::io::Error::other)?;
    } else {
        service.stop().map_err(std::io::Error::other)?;
    }
    if std::env::var("REMOVENT_NO_TRAY").as_deref() != Ok("1") {
        super::tray::start_tray(paths, bundle)?;
    }
    Ok(())
}

/// Locate the running .app bundle by walking up from the current executable
/// (`<bundle>.app/Contents/MacOS/removent`); None outside a bundle (dev runs).
#[cfg(target_os = "macos")]
pub(super) fn current_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .map(|p| p.to_path_buf())
}

#[cfg(not(target_os = "macos"))]
pub fn run_install(
    _generation: u64,
    shared: Arc<Mutex<UpdateShared>>,
    events: std::sync::mpsc::Sender<UiEvent>,
    _paths: DataPaths,
) {
    set_status(
        &shared,
        &events,
        UpdateStatus::Failed("Install the Windows or Linux package manually".into()),
    );
}
