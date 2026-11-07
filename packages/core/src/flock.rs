//! Data-dir mutex lock (architecture.md §8: flock; a second instance is refused or runs read-only).

use crate::error::{CoreError, Result};
use crate::paths::DataPaths;
use std::fs::File;
use std::os::unix::io::AsRawFd;

pub struct DataDirLock {
    _file: File,
}

impl DataDirLock {
    pub fn acquire(paths: &DataPaths) -> Result<Self> {
        paths.ensure_layout()?;
        Self::acquire_at(&paths.lock_file())
    }

    /// Lock at the given path (the daemon etc. use a separate lock file).
    pub fn acquire_at(lock_file: &std::path::Path) -> Result<Self> {
        Self::lock_at(lock_file, false)
    }

    /// Serialize short initialization transactions shared by app and daemon.
    pub fn acquire_blocking(lock_file: &std::path::Path) -> Result<Self> {
        Self::lock_at(lock_file, true)
    }

    fn lock_at(lock_file: &std::path::Path, blocking: bool) -> Result<Self> {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        if let Some(parent) = lock_file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(lock_file)?;
        let meta = file.metadata()?;
        if !meta.is_file()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.nlink() != 1
            || meta.mode() & 0o022 != 0
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Unsafe process lock file",
            )
            .into());
        }
        let flags = libc::LOCK_EX | if blocking { 0 } else { libc::LOCK_NB };
        loop {
            let rc = unsafe { libc::flock(file.as_raw_fd(), flags) };
            if rc == 0 {
                break;
            }
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            if error.kind() == std::io::ErrorKind::WouldBlock {
                return Err(CoreError::AlreadyRunning);
            }
            return Err(error.into());
        }
        Ok(Self { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn second_acquire_fails() {
        let dir = tempdir().unwrap();
        let p = DataPaths {
            root: dir.path().to_path_buf(),
        };
        let _g1 = DataDirLock::acquire(&p).unwrap();
        assert!(matches!(
            DataDirLock::acquire(&p),
            Err(CoreError::AlreadyRunning)
        ));
        drop(_g1);
        let _g2 = DataDirLock::acquire(&p).unwrap();
    }

    #[test]
    fn a_fifo_cannot_hang_service_or_recovery_startup() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("lock");
        let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let start = std::time::Instant::now();
        assert!(DataDirLock::acquire_at(&path).is_err());
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
    }
}
