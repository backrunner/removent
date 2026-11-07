use super::{
    health, io,
    journal::{Journal, Phase},
    preflight, registration,
};
use anyhow::{Result, ensure};
use removent_core::{
    DataPaths,
    service::Service,
    update_gate::{BundleMaintenanceLease, MaintenanceLease, bundle_marker},
};
use std::{fs, path::Path, time::Duration};

fn service(paths: &DataPaths, bundle: &Path) -> Result<Service> {
    Service::new(paths.clone(), bundle.join("Contents/MacOS/removentd"))
}
fn marker(paths: &DataPaths) -> std::path::PathBuf {
    paths.run_dir().join("update-maintenance")
}

fn clear_markers(paths: &DataPaths, bundle: &Path) -> Result<()> {
    let global = bundle_marker(bundle)?;
    if global.exists() {
        let owner: std::path::PathBuf = io::load(&global)?;
        ensure!(
            owner == paths.root.canonicalize()?,
            "Another data directory owns the recovery marker"
        );
        io::remove(&global)?;
    }
    io::remove(&marker(paths))
}

pub(super) fn rollback(
    paths: &DataPaths,
    tx: &mut Journal,
    log: &Path,
    lease: &MaintenanceLease,
) -> Result<()> {
    // Validate the recovery copy BEFORE deleting or stopping anything. A disk
    // error affecting the backup must never destroy a still-usable installation.
    let restore_from = if tx.backup.exists() {
        &tx.backup
    } else {
        &tx.bundle
    };
    preflight::verify(restore_from)?;
    ensure!(
        crate::bundle_version(restore_from).map_err(anyhow::Error::msg)? == tx.previous,
        "Known-good rollback copy is unavailable; installation left untouched"
    );
    let service = service(paths, &tx.bundle)?;
    service.quiesce_for_update(lease)?;
    // Stop only our own tray. Its known launch path is unchanged after rename.
    crate::tray::stop_tray(paths, &tx.bundle)?;
    let restored_journal = tx.restore(log);
    // Restoration must verify the prior known-good copy before running it.
    preflight::verify(&tx.bundle)?;
    ensure!(
        crate::bundle_version(&tx.bundle).map_err(anyhow::Error::msg)? == tx.previous,
        "Rollback version mismatch"
    );
    if tx.resume_host {
        service.resume_after_update(lease)?;
    }
    clear_markers(paths, &tx.bundle)?;
    if tx.resume_tray {
        crate::tray::start_tray(paths, &tx.bundle)?;
    }
    if let Err(error) = restored_journal {
        // The old program is already back and verified. A full disk must not
        // prevent restarting it merely because the final journal write failed.
        tracing::warn!(%error, "rollback restored service; journal repair pending");
    }
    Ok(())
}

/// Recovery precedes settings, feed access and opt-out: a disabled updater must
/// still finish restoring a transaction that already stopped the host.
pub fn recover(paths: &DataPaths, bundle: &Path) -> Result<()> {
    let log = registration::directory(paths).join("journal.json");
    if !log.exists() {
        ensure!(
            !marker(paths).exists(),
            "Update marker has no recovery journal"
        );
        return Ok(());
    }
    let mut tx: Journal = io::load(&log)?;
    tx.validate(bundle)?;
    if matches!(tx.phase, Phase::Committed | Phase::RolledBack)
        && !marker(paths).exists()
        && !bundle_marker(bundle)?.exists()
    {
        return Ok(());
    }
    let _bundle_lock = BundleMaintenanceLease::acquire(bundle)?;
    if !tx.backup.exists()
        && !marker(paths).exists()
        && !bundle_marker(bundle)?.exists()
        && crate::bundle_version(bundle).ok().as_deref() == Some(tx.previous.as_str())
    {
        preflight::verify(bundle)?;
        tx.phase = Phase::RolledBack;
        let _ = tx.save(&log);
        let _ = super::reject_version(paths, &tx.version);
        return Ok(());
    }
    if tx.phase == Phase::Committed {
        clear_markers(paths, &tx.bundle)?;
        return Ok(());
    }
    if tx.phase == Phase::RolledBack && !marker(paths).exists() {
        return Ok(());
    }
    let lease = MaintenanceLease::acquire(paths)?;
    rollback(paths, &mut tx, &log, &lease)?;
    super::reject_version(paths, &tx.version)?;
    super::status(
        paths,
        "rolled_back",
        Some(&tx.version),
        "The previous version was restored. A newer repair update can still be installed.",
    )?;
    Ok(())
}

pub fn install(
    paths: &DataPaths,
    bundle: &Path,
    staged: &Path,
    version: &str,
    reopen: bool,
    policy: &crate::UpdatePolicy,
) -> Result<()> {
    // Expensive and permission-sensitive work happens before stopping anything.
    preflight::installation(bundle, staged)?;
    let current = crate::bundle_version(bundle).map_err(anyhow::Error::msg)?;
    ensure!(
        crate::is_newer(version, &current),
        "The installed app is already newer"
    );
    let _bundle_lock = BundleMaintenanceLease::acquire(bundle)?;
    ensure!(
        !bundle_marker(bundle)?.try_exists()?,
        "Another data directory has unfinished installation recovery"
    );
    let dir = registration::directory(paths);
    let log = dir.join("journal.json");
    let mut tx = Journal::new(bundle, version, &current);
    tx.validate(bundle)?;
    if tx.incoming.exists() {
        fs::remove_dir_all(&tx.incoming)?;
    }
    preflight::copy(staged, &tx.incoming)?;
    ensure!(
        crate::bundle_version(&tx.incoming).map_err(anyhow::Error::msg)? == version,
        "Copied candidate version changed"
    );
    // Prove that the real desktop can render and remain alive before downtime.
    health::desktop_smoke(&tx.incoming, version)?;
    let lease = MaintenanceLease::acquire(paths)?;
    let latest = removent_core::Settings::load_strict(paths)?;
    ensure!(
        crate::UpdatePolicy::from_settings(&latest) == *policy
            && (reopen || (latest.update_check_enabled && latest.update_auto_install)),
        "Update preferences changed; installation cancelled"
    );
    let svc = service(paths, bundle)?;
    let before = svc.status()?;
    ensure!(
        before.managed || !before.reachable,
        "Stop the unmanaged development host before updating"
    );
    if before.reachable {
        let aware = match removent_core::DataDirLock::acquire_at(
            &paths.run_dir().join("update-aware-daemon.lock"),
        ) {
            Err(removent_core::CoreError::AlreadyRunning) => true,
            Ok(_) => false,
            Err(error) => return Err(error.into()),
        };
        ensure!(
            aware,
            "The running host predates safe idle updates; restart the host from General settings before automatic installation"
        );
    }
    tx.resume_host = !before.stopped_by_user && (before.managed || before.reachable);
    tx.baseline = health::read(paths).ok();
    if let Some(baseline) = &tx.baseline {
        ensure!(
            baseline.status.sessions.is_empty(),
            "A host session is still active"
        );
        ensure!(
            !baseline.status.running
                || (baseline.status.screen_recording_granted
                    && baseline.status.accessibility_granted),
            "Grant Screen Recording and Accessibility before unattended installation"
        );
    }
    if tx.baseline.is_none() && paths.device_cert().exists() {
        // Only the public certificate is read; credentials never leave storage.
        let fingerprint = crate::sha256_file(&paths.device_cert()).map_err(anyhow::Error::msg)?;
        tx.baseline = Some(health::Health {
            version: current.clone(),
            pid: 0,
            status: removent_core::ipc::StatusReport {
                fp_short: fingerprint[..16].into(),
                ..Default::default()
            },
        });
    }
    tx.resume_tray = crate::tray::tray_process(paths, bundle)?.is_some();
    tx.reopen = reopen;
    if let Some(baseline) = tx.baseline.as_mut() {
        // Health history needs identity/grants, never an invitation secret.
        baseline.status.pending_pin = None;
    }
    // Retire only a backup belonging to an already completed transaction.
    if tx.backup.exists() {
        let previous: Journal = io::load(&log)?;
        previous.validate(bundle)?;
        ensure!(
            previous.phase == Phase::Committed,
            "Previous recovery is unfinished"
        );
        fs::remove_dir_all(&tx.backup)?;
    }
    tx.save(&log)?;
    io::save(&bundle_marker(bundle)?, &paths.root.canonicalize()?)?;
    io::write(&marker(paths), b"Recovery agent owns this installation\n")?;
    let attempt = (|| -> Result<()> {
        svc.quiesce_for_update(&lease)?;
        crate::tray::stop_tray(paths, bundle)?;
        // Last verification after all application processes have quiesced.
        preflight::verify(&tx.incoming)?;
        tx.publish(&log)?;
        if tx.resume_host && !removent_core::service_intent::is_stopped(paths)? {
            svc.resume_after_update(&lease)?;
            health::probation(
                paths,
                version,
                tx.baseline.as_ref(),
                Duration::from_secs(60),
            )?;
        } else {
            // Respect an explicit host stop; probe the executable on disposable data.
            health::daemon_smoke(bundle, version)?;
        }
        tx.phase = Phase::Committed;
        tx.save(&log)?;
        clear_markers(paths, &tx.bundle)?;
        Ok(())
    })();
    if let Err(error) = attempt {
        // Quarantine before recovery so a restart never loops over a bad version.
        super::reject_version(paths, version)?;
        rollback(paths, &mut tx, &log, &lease)?;
        return Err(error.context("Update failed; previous version restored"));
    }
    // Restart optional UI only after the host has passed probation. A UI launch
    // error cannot invalidate or stop an already verified host.
    drop(lease);
    if tx.resume_tray {
        crate::tray::start_tray(paths, bundle)?;
    }
    if reopen {
        super::open_desktop(paths, bundle)?;
    }
    super::supervisor::stage(paths, bundle)?;
    let system = removent_core::system_service::status()?;
    if system.installed && system.version != version {
        super::status(
            paths,
            "needs_admin",
            Some(version),
            "The user-session host was updated. The login-window host keeps its working version; open General settings to update it with administrator approval.",
        )?;
    } else {
        super::status(
            paths,
            "installed",
            Some(version),
            "Update installed and startup checks passed",
        )?;
    }
    Ok(())
}
