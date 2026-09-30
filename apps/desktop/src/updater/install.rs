use super::*;

/// Swap the staged .app into place and relaunch. Runs on a blocking thread;
/// the process exits on success.
pub fn run_install(
    generation: u64,
    shared: Arc<Mutex<UpdateShared>>,
    events: std::sync::mpsc::Sender<UiEvent>,
    daemon_req: DaemonReq,
) {
    // Concurrency guard (double-click on "Install"): claim the ReadyToInstall →
    // Swapping transition inside one lock; a second call returns instead of
    // racing swap_bundle (whose backup cleanup would delete the first swap's
    // rollback copy).
    let (staged, version) = {
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
        (staged, version)
    };
    let resume_service =
        match removent_core::service_intent::is_stopped(&removent_core::DataPaths::resolve()) {
            Ok(stopped) => !stopped,
            Err(error) => {
                set_status(&shared, &events, UpdateStatus::Failed(error.to_string()));
                return;
            }
        };
    match swap_bundle(&staged, &version) {
        Ok(bundle) => {
            // Stop the old daemon before relaunching: it ships inside the
            // bundle, and a daemon spawned by the app (not launchd) would
            // otherwise keep running the old version while the new app
            // connects to it. launchd restarts it via the kickstart in
            // relaunch; otherwise the new app re-spawns it on demand.
            if let Some(tx) = daemon_req.lock().unwrap().as_ref() {
                let _ = tx.send(removent_core::ipc::IpcRequest::Shutdown);
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
            set_status(&shared, &events, UpdateStatus::Relaunching);
            if let Err(error) = relaunch(&bundle, resume_service) {
                let backup = bundle.with_extension("app.old");
                let failed = bundle.with_extension("app.failed");
                let restored = std::fs::rename(&bundle, &failed)
                    .and_then(|()| std::fs::rename(&backup, &bundle));
                let reason = if restored.is_ok() {
                    let _ = std::fs::remove_dir_all(failed);
                    // The replacement daemon may already have started. Put
                    // the restored bundle's server back under the same job.
                    #[cfg(target_os = "macos")]
                    if resume_service
                        && let Ok(service) = removent_core::service::Service::new(
                            removent_core::DataPaths::resolve(),
                            bundle.join("Contents/MacOS/removentd"),
                        )
                    {
                        let _ = service.restart();
                    }
                    t!("update.err.swap", err = error.to_string()).to_string()
                } else {
                    t!(
                        "update.err.swap_rollback",
                        err = error.to_string(),
                        backup = backup.display().to_string()
                    )
                    .to_string()
                };
                set_status(&shared, &events, UpdateStatus::Failed(reason));
            }
        }
        Err(reason) => set_status(&shared, &events, UpdateStatus::Failed(reason)),
    }
}

/// mv the running bundle aside (`Removent.app.old`) and move the staged bundle
/// in, rolling back when the new bundle cannot be moved into place.
pub(super) fn swap_bundle(staged_app: &Path, expected_version: &str) -> Result<PathBuf, String> {
    let Some(bundle) = current_bundle() else {
        // Not running from a .app (cargo run / target dir): refuse to install.
        return Err(t!("update.err.not_in_bundle").to_string());
    };
    // Revalidate immediately before replacement. The bundle on disk may have
    // been updated externally since this process started or the ZIP was staged.
    if !is_newer(expected_version, &bundle_version(&bundle)?) {
        return Err(t!("update.err.not_upgrade").to_string());
    }
    if bundle_version(staged_app)? != expected_version {
        return Err(t!("update.err.unpack", err = "staged version changed").to_string());
    }
    codesign_verify(staged_app)?;
    replace_bundle(staged_app, &bundle)?;
    Ok(bundle)
}

/// Stage on the destination volume before moving the running app. A failed
/// cross-volume copy must never leave a partial new bundle blocking rollback.
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

/// Restart the daemon (it ships inside the bundle; best-effort — it may not be
/// loaded at all), open the new app and exit only if Launch Services accepts it.
pub(super) fn relaunch(bundle: &Path, resume_service: bool) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    if resume_service {
        removent_core::service::Service::new(
            removent_core::DataPaths::resolve(),
            bundle.join("Contents/MacOS/removentd"),
        )
        .and_then(|service| service.restart())
        .map_err(std::io::Error::other)?;
    }
    let opened = Command::new("/usr/bin/open")
        .arg("-n")
        .arg(bundle)
        .status()?;
    if !opened.success() {
        return Err(std::io::Error::other(format!("open exit {opened}")));
    }
    std::process::exit(0);
}

/// Locate the running .app bundle by walking up from the current executable
/// (`<bundle>.app/Contents/MacOS/removent`); None outside a bundle (dev runs).
pub(super) fn current_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .map(|p| p.to_path_buf())
}

/// Remove the `Removent.app.old` backup left by a previous update (called once
/// at startup — reaching this code means the new version launched fine).
pub fn cleanup_stale_backup() {
    std::thread::Builder::new()
        .name("update-cleanup".into())
        .spawn(|| {
            let Some(bundle) = current_bundle() else {
                return;
            };
            let backup = bundle.with_extension("app.old");
            if backup.exists() {
                let _ = std::fs::remove_dir_all(&backup);
            }
        })
        .expect("spawn update cleanup");
}
