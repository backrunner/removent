//! Cross-process admission gate: sessions hold shared leases; replacement holds
//! the exclusive lease. File existence is never interpreted as a live lock.
use crate::DataPaths;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub struct ActivityLease {
    _file: File,
    _bundle: Option<File>,
}
pub struct MaintenanceLease {
    _file: File,
}
pub struct BundleMaintenanceLease {
    _file: File,
}

/// Same installation, different custom data roots: still one update boundary.
/// Locks live in user-owned support data, never beside a read-only app.
pub fn bundle_lock_file(bundle: &Path) -> io::Result<PathBuf> {
    use sha2::{Digest, Sha256};
    let home = std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME unavailable"))?;
    let root = PathBuf::from(home).join("Library/Application Support/removent/update-locks");
    std::fs::create_dir_all(&root)?;
    let meta = std::fs::symlink_metadata(&root)?;
    if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o022 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Unsafe installation lock directory",
        ));
    }
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?;
    let parent = bundle
        .parent()
        .ok_or_else(|| io::Error::other("Bundle parent missing"))?
        .canonicalize()?;
    let normalized = parent.join(
        bundle
            .file_name()
            .ok_or_else(|| io::Error::other("Bundle name missing"))?,
    );
    Ok(root.join(hex::encode(Sha256::digest(
        normalized.as_os_str().as_encoded_bytes(),
    ))))
}

pub fn bundle_marker(bundle: &Path) -> io::Result<PathBuf> {
    Ok(bundle_lock_file(bundle)?.with_extension("maintenance"))
}

fn current_bundle() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    executable
        .ancestors()
        .filter(|p| p.extension().is_some_and(|e| e == "app"))
        .last()
        .map(Path::to_path_buf)
}

impl BundleMaintenanceLease {
    pub fn acquire(bundle: &Path) -> io::Result<Self> {
        Ok(Self {
            _file: acquire_file(&bundle_lock_file(bundle)?, true)?,
        })
    }
}

fn acquire(paths: &DataPaths, exclusive: bool) -> io::Result<File> {
    std::fs::create_dir_all(paths.run_dir())?;
    acquire_file(&paths.run_dir().join("update-activity.lock"), exclusive)
}

fn acquire_file(path: &Path, exclusive: bool) -> io::Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.nlink() != 1
        || meta.mode() & 0o022 != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unsafe update admission lock",
        ));
    }
    loop {
        let flags = if exclusive {
            libc::LOCK_EX
        } else {
            libc::LOCK_SH
        } | libc::LOCK_NB;
        if unsafe { libc::flock(file.as_raw_fd(), flags) } == 0 {
            return Ok(file);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

impl ActivityLease {
    pub fn acquire(paths: &DataPaths) -> io::Result<Self> {
        let file = acquire(paths, false)?;
        let bundle = current_bundle()
            .map(|bundle| -> io::Result<File> {
                let file = acquire_file(&bundle_lock_file(&bundle)?, false)?;
                if bundle_marker(&bundle)?.try_exists()? {
                    return Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "Installation recovery is pending",
                    ));
                }
                Ok(file)
            })
            .transpose()?;
        let lease = Self {
            _file: file,
            _bundle: bundle,
        };
        if paths.run_dir().join("update-maintenance").try_exists()? {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Update recovery is pending",
            ));
        }
        Ok(lease)
    }
}
impl MaintenanceLease {
    pub fn acquire(paths: &DataPaths) -> io::Result<Self> {
        Ok(Self {
            _file: acquire(paths, true)?,
        })
    }
}

/// Errors fail closed: a damaged gate must not permit a watchdog to race a swap.
pub fn maintenance_active(paths: &DataPaths) -> bool {
    ActivityLease::acquire(paths).is_err()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn admission_and_maintenance_are_atomic_and_recover_on_owner_exit() {
        let temp = tempfile::tempdir().unwrap();
        let paths = DataPaths {
            root: temp.path().into(),
        };
        let a = ActivityLease::acquire(&paths).unwrap();
        let b = ActivityLease::acquire(&paths).unwrap();
        assert!(MaintenanceLease::acquire(&paths).is_err());
        drop(a);
        drop(b);
        let maintenance = MaintenanceLease::acquire(&paths).unwrap();
        assert!(ActivityLease::acquire(&paths).is_err());
        assert!(MaintenanceLease::acquire(&paths).is_err());
        drop(maintenance);
        assert!(ActivityLease::acquire(&paths).is_ok());
    }
    #[test]
    fn a_symlink_cannot_redirect_the_gate() {
        let temp = tempfile::tempdir().unwrap();
        let paths = DataPaths {
            root: temp.path().into(),
        };
        std::fs::create_dir_all(paths.run_dir()).unwrap();
        std::os::unix::fs::symlink(
            temp.path().join("target"),
            paths.run_dir().join("update-activity.lock"),
        )
        .unwrap();
        assert!(ActivityLease::acquire(&paths).is_err());
        assert!(MaintenanceLease::acquire(&paths).is_err());
    }

    #[test]
    fn interrupted_update_remains_closed_after_the_process_lease_is_gone() {
        let temp = tempfile::tempdir().unwrap();
        let paths = DataPaths {
            root: temp.path().into(),
        };
        let maintenance = MaintenanceLease::acquire(&paths).unwrap();
        std::fs::write(paths.run_dir().join("update-maintenance"), b"recover").unwrap();
        drop(maintenance);
        assert!(ActivityLease::acquire(&paths).is_err());
        assert!(MaintenanceLease::acquire(&paths).is_ok());
        assert!(crate::Settings::default().save(&paths).is_err());
        std::fs::remove_file(paths.run_dir().join("update-maintenance")).unwrap();
        assert!(crate::Settings::default().save(&paths).is_ok());
    }

    #[test]
    fn installation_gate_serializes_distinct_data_directories() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("bundle.lock");
        let a = acquire_file(&path, false).unwrap();
        let b = acquire_file(&path, false).unwrap();
        assert!(acquire_file(&path, true).is_err());
        drop(a);
        drop(b);
        let installer = acquire_file(&path, true).unwrap();
        assert!(acquire_file(&path, false).is_err());
        drop(installer);
        assert!(acquire_file(&path, false).is_ok());
    }
}
