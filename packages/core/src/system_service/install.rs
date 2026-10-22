use super::*;
use crate::DataDirLock;
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    time::{Duration, Instant},
};

fn installation_lock() -> Result<DataDirLock> {
    ensure!(
        unsafe { libc::geteuid() } == 0,
        "Administrator authorization is required to manage startup before login"
    );
    let parent = Path::new(ROOT).parent().unwrap();
    security::safe_directory(parent)?;
    security::safe_directory(Path::new(PLIST).parent().unwrap())?;
    let lock = parent.join(".login-window-install.lock");
    if lock.try_exists()? {
        security::verify_root_owned(&lock)?;
    }
    Ok(DataDirLock::acquire_blocking(&lock)?)
}

/// Stop through private IPC and keep launchd from relaunching while a root-owned
/// snapshot is replaced. No PID guesses, process-name kills, or user logout.
fn stop(root: &Path) -> Result<Option<DataDirLock>> {
    if !root.try_exists()? {
        return Ok(None);
    }
    security::verify_root_owned(root)?;
    let paths = DataPaths {
        root: root.join("data"),
    };
    security::verify_root_owned(&paths.root)?;
    crate::service_intent::set_stopped(&paths, true)?;
    if let Ok(mut socket) = std::os::unix::net::UnixStream::connect(paths.daemon_socket()) {
        socket.set_write_timeout(Some(Duration::from_secs(2)))?;
        let mut request = serde_json::to_vec(&crate::ipc::IpcRequest::Shutdown)?;
        request.push(b'\n');
        socket.write_all(&request)?;
    }
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        match DataDirLock::acquire_at(&paths.daemon_lock_file()) {
            Ok(lock) => return Ok(Some(lock)),
            Err(crate::CoreError::AlreadyRunning) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => {
                return Err(error)
                    .context("System host is still running; retry after its session ends");
            }
        }
    }
}

pub(super) fn disable() -> Result<()> {
    let _installation = installation_lock()?;
    let root = Path::new(ROOT);
    let plist = Path::new(PLIST);
    if plist.try_exists()? {
        security::verify_root_owned(plist)?;
        fs::remove_file(plist)?;
    }
    let _host = stop(root)?;
    Ok(())
}

pub(super) fn install(bundle: &Path, source: &DataPaths, enable: bool) -> Result<()> {
    let _installation = installation_lock()?;
    security::verify_bundle(bundle)?;
    let root = Path::new(ROOT);
    let plist = Path::new(PLIST);
    ensure!(
        enable || root.try_exists()?,
        "Install the system service before updating it"
    );
    let enabled = enable || plist.try_exists()?;
    if root.try_exists()? {
        security::verify_root_owned(root)?;
    }
    if plist.try_exists()? {
        security::verify_root_owned(plist)?;
    }
    let staged = root.parent().unwrap().join(format!(
        ".login-window-{}-{}",
        std::process::id(),
        crate::peers::now_unix()
    ));
    fs::create_dir(&staged)?;
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o700))?;
    let result = (|| -> Result<()> {
        let copy = staged.join("Removent.app");
        security::checked("/usr/bin/ditto", &[bundle.as_os_str(), copy.as_os_str()])?;
        security::checked("/bin/chmod", &["-RN".as_ref(), staged.as_os_str()])?;
        security::checked(
            "/usr/sbin/chown",
            &["-Rh".as_ref(), "0:0".as_ref(), staged.as_os_str()],
        )?;
        security::secure_bundle_tree(&copy, &copy)?;
        security::verify_bundle(&copy)?;
        let status = snapshot::stage(source, &staged)?;
        if root.try_exists()? {
            // A different user's identity must not silently take over the
            // machine-wide service, even when the app is run by an admin.
            use sha2::{Digest, Sha256};
            let cert = security::read_regular(&root.join("data/identity/device.crt"))?;
            ensure!(
                hex::encode(Sha256::digest(cert)) == status.host_fingerprint,
                "This system host belongs to another identity; manage it from the original Removent profile"
            );
        }
        let old_stopped = root.join("data/run/service-stopped").try_exists()?;
        let result = (|| {
            let _host = stop(root)?;
            publish(&staged, root, plist, enabled)
        })();
        if result.is_err() && root.try_exists()? && !old_stopped {
            crate::service_intent::set_stopped(
                &DataPaths {
                    root: root.join("data"),
                },
                false,
            )?;
        }
        result
    })();
    if staged.exists() {
        let _ = fs::remove_dir_all(staged);
    }
    result
}

/// Stage before mutating, and restore both the old installation and registration
/// on failure. The root-owned parent prevents unprivileged replacement races.
pub(super) fn publish(staged: &Path, root: &Path, plist: &Path, enabled: bool) -> Result<()> {
    let backup = root.with_file_name(format!(".login-window-previous-{}", std::process::id()));
    ensure!(
        !backup.try_exists()?,
        "Previous system host backup requires inspection before updating"
    );
    let previous_plist = match fs::read(plist) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let had_root = root.try_exists()?;
    if had_root {
        fs::rename(root, &backup)?;
    }
    if let Err(error) = fs::rename(staged, root) {
        if had_root {
            fs::rename(&backup, root)?;
        }
        return Err(error.into());
    }
    let result = (|| -> Result<()> {
        if enabled {
            crate::settings::atomic_write(&plist.to_path_buf(), definition(root).as_bytes())?;
            fs::set_permissions(plist, fs::Permissions::from_mode(0o644))?;
            crate::service_intent::set_stopped(
                &DataPaths {
                    root: root.join("data"),
                },
                false,
            )?;
        } else if plist.try_exists()? {
            fs::remove_file(plist)?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        fs::remove_dir_all(root)?;
        if had_root {
            fs::rename(&backup, root)?;
        }
        match previous_plist {
            Some(bytes) => {
                crate::settings::atomic_write(&plist.to_path_buf(), &bytes)?;
                fs::set_permissions(plist, fs::Permissions::from_mode(0o644))?;
            }
            None if plist.is_file() => fs::remove_file(plist)?,
            None => {}
        }
        return Err(error);
    }
    if had_root {
        fs::remove_dir_all(backup)?;
    }
    Ok(())
}
