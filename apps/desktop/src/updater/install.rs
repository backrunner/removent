use super::*;

/// Swap the staged .app into place and relaunch. Runs on a blocking thread;
/// the process exits on success.
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
    let resume_service = match removent_core::service_intent::is_stopped(&paths) {
        Ok(stopped) => !stopped,
        Err(error) => {
            set_status(&shared, &events, UpdateStatus::Failed(error.to_string()));
            return;
        }
    };
    #[cfg(target_os = "macos")]
    let tray_was_running = match current_bundle()
        .map(|bundle| super::tray::stop_tray(&paths, &bundle))
        .transpose()
    {
        Ok(running) => running.unwrap_or(false),
        Err(error) => {
            set_status(
                &shared,
                &events,
                UpdateStatus::Failed(t!("update.err.swap", err = error.to_string()).to_string()),
            );
            return;
        }
    };
    match swap_bundle(&staged, &version) {
        Ok(bundle) => {
            set_status(&shared, &events, UpdateStatus::Relaunching);
            if let Err(error) = relaunch(&bundle, &paths, resume_service) {
                // The replacement tray must release its lock before restoring
                // the old bundle, otherwise rollback leaves a new tray running.
                #[cfg(target_os = "macos")]
                if let Err(stop_error) = super::tray::stop_tray(&paths, &bundle) {
                    set_status(
                        &shared,
                        &events,
                        UpdateStatus::Failed(
                            t!(
                                "update.err.swap_rollback",
                                err = format!("{error}; {stop_error}"),
                                backup = bundle.with_extension("app.old").display().to_string(),
                            )
                            .to_string(),
                        ),
                    );
                    return;
                }
                let backup = bundle.with_extension("app.old");
                let failed = bundle.with_extension("app.failed");
                let restored = std::fs::rename(&bundle, &failed)
                    .and_then(|()| std::fs::rename(&backup, &bundle));
                let reason = if restored.is_ok() {
                    let _ = std::fs::remove_dir_all(failed);
                    // Restore all background components, retaining manual stop.
                    #[cfg(target_os = "macos")]
                    if let Err(recovery) = restart_background(&bundle, &paths, resume_service) {
                        set_status(
                            &shared,
                            &events,
                            UpdateStatus::Failed(
                                t!("update.err.swap", err = format!("{error}; {recovery}"),)
                                    .to_string(),
                            ),
                        );
                        return;
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
        Err(reason) => {
            #[cfg(target_os = "macos")]
            let reason = if tray_was_running
                && let Some(bundle) = current_bundle()
                && let Err(error) = super::tray::start_tray(&paths, &bundle)
            {
                format!("{reason} ({error})")
            } else {
                reason
            };
            set_status(&shared, &events, UpdateStatus::Failed(reason));
        }
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

/// Service::restart serializes against watchdogs, unloads KeepAlive and waits
/// for the old daemon lock to release before launching the new executable.
#[cfg(target_os = "macos")]
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

/// Verify the new background processes before opening the desktop. Retain the
/// data directory when Launch Services creates the replacement app process.
pub(super) fn relaunch(
    bundle: &Path,
    paths: &DataPaths,
    resume_service: bool,
) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    restart_background(bundle, paths, resume_service)?;
    #[cfg(not(target_os = "macos"))]
    let _ = resume_service;
    let opened = Command::new("/usr/bin/open")
        .arg("-n")
        .arg("--env")
        .arg(format!("REMOVENT_DATA_DIR={}", paths.root.display()))
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
