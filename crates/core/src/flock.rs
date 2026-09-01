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
        if let Some(parent) = lock_file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(lock_file)?;
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            return Err(CoreError::AlreadyRunning);
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
}
