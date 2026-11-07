//! Replace the independent menu bar process, including trays from older releases.
use super::*;
use removent_core::{CoreError, DataDirLock};
use std::io;

fn tray_bundle(bundle: &Path) -> PathBuf {
    bundle.join("Contents/Helpers/RemoventTray.app")
}

fn locked(paths: &DataPaths) -> io::Result<bool> {
    match DataDirLock::acquire_at(&paths.root.join(".tray.lock")) {
        Ok(lock) => {
            drop(lock);
            Ok(false)
        }
        Err(CoreError::AlreadyRunning) => Ok(true),
        Err(error) => Err(io::Error::other(error)),
    }
}

/// Include owner, start time and executable so PID reuse cannot target another
/// process. `comm` retains the full path when -ww is supplied, including spaces.
fn fingerprint(pid: i32, executable: &Path) -> io::Result<Option<String>> {
    let output = Command::new("/bin/ps")
        .args(["-ww", "-p", &pid.to_string(), "-o", "uid=,lstart=,comm="])
        .output()?;
    if !output.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let mut remainder = text.as_str();
    let owner = remainder.split_whitespace().next().unwrap_or_default();
    if owner != unsafe { libc::getuid() }.to_string() {
        return Ok(None);
    }
    // UID followed by the five fields in lstart; preserve spaces in the path.
    for _ in 0..6 {
        let Some((_, rest)) = remainder.split_once(char::is_whitespace) else {
            return Ok(None);
        };
        remainder = rest.trim_start();
    }
    Ok((Path::new(remainder) == executable).then_some(text))
}

pub(super) fn tray_process(paths: &DataPaths, bundle: &Path) -> io::Result<Option<(i32, String)>> {
    // Older trays only expose a flock, so discover file holders rather than
    // relying on a new IPC command or killing every process with this name.
    let output = Command::new("/usr/sbin/lsof")
        .args([
            "-nP",
            "-t",
            "-a",
            "-u",
            &unsafe { libc::getuid() }.to_string(),
        ])
        .arg(paths.root.join(".tray.lock"))
        .output()?;
    if !output.status.success() && !output.stdout.is_empty() {
        return Err(io::Error::other(
            "Could not identify the running menu bar app",
        ));
    }
    let executable = tray_bundle(bundle).join("Contents/MacOS/RemoventTray");
    let mut found = None;
    let mut pids: Vec<i32> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.parse().ok())
        .collect();
    pids.sort_unstable();
    pids.dedup();
    for pid in pids {
        if let Some(identity) = fingerprint(pid, &executable)? {
            if found.is_some() {
                return Err(io::Error::other(
                    "Multiple menu bar processes hold the update lock",
                ));
            }
            found = Some((pid, identity));
        }
    }
    Ok(found)
}

/// Wait for both process exit and singleton lock release before replacing its
/// bundle. An unrelated tray/data directory is never signalled.
pub(super) fn stop_tray(paths: &DataPaths, bundle: &Path) -> io::Result<bool> {
    if !locked(paths)? {
        return Ok(false);
    }
    let (pid, identity) = tray_process(paths, bundle)?.ok_or_else(|| {
        io::Error::other("Could not identify the running menu bar app; quit it and retry")
    })?;
    let executable = tray_bundle(bundle).join("Contents/MacOS/RemoventTray");
    if fingerprint(pid, &executable)?.as_ref() != Some(&identity) {
        return Err(io::Error::other(
            "The menu bar process changed; retry the update",
        ));
    }
    if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let exited = fingerprint(pid, &executable)?.as_ref() != Some(&identity);
        if exited && !locked(paths)? {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(
                "The menu bar app is still shutting down; quit it and retry",
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Older installers leave their tray alive after moving its bundle to .old.
/// ps retains the launch path, but lsof reports the executable actually mapped
/// by that process, even after startup cleanup has removed the backup directory.
pub(super) fn refresh_tray(paths: &DataPaths, bundle: &Path) -> io::Result<bool> {
    if !locked(paths)? {
        return Ok(false);
    }
    let Some((pid, _)) = tray_process(paths, bundle)? else {
        return Ok(false);
    };
    let output = Command::new("/usr/sbin/lsof")
        .args(["-nP", "-a", "-p", &pid.to_string(), "-d", "txt", "-Fn"])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(
            "Could not inspect the running menu bar version",
        ));
    }
    let old_executable =
        tray_bundle(&bundle.with_extension("app.old")).join("Contents/MacOS/RemoventTray");
    let stale = String::from_utf8_lossy(&output.stdout).lines().any(|line| {
        line.strip_prefix('n')
            .is_some_and(|name| Path::new(name) == old_executable)
    });
    if stale {
        return stop_tray(paths, bundle);
    }
    Ok(false)
}

pub fn refresh_stale_tray(paths: &DataPaths) -> io::Result<bool> {
    let Some(bundle) = super::install::current_bundle() else {
        return Ok(false);
    };
    refresh_tray(paths, &bundle)
}

pub(super) fn start_tray(paths: &DataPaths, bundle: &Path) -> io::Result<()> {
    let tray = tray_bundle(bundle);
    if !tray.join("Contents/MacOS/RemoventTray").is_file() {
        return Err(io::Error::other("The updated menu bar app is missing"));
    }
    let mut command = Command::new("/usr/bin/open");
    command
        .args(["-g", "-n", "--env"])
        .arg(format!("REMOVENT_DATA_DIR={}", paths.root.display()));
    if std::env::var("REMOVENT_TRAY_NO_ALERTS").as_deref() == Ok("1") {
        command.args(["--env", "REMOVENT_TRAY_NO_ALERTS=1"]);
    }
    let opened = command.arg(tray).status()?;
    if !opened.success() {
        return Err(io::Error::other(format!("open menu bar app exit {opened}")));
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if locked(paths)? && tray_process(paths, bundle)?.is_some() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(
                "The updated menu bar app did not start; retry the update",
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
#[path = "tray_tests.rs"]
mod tests;
