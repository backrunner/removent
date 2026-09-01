//! `userdata/` data directory resolution (architecture.md §8).
//!
//! Resolution order:
//! 1. `REMOVENT_DATA_DIR` environment variable
//! 2. Debug builds: `userdata/` under the workspace root (compile-time path)
//! 3. Release builds: `~/Library/Application Support/removent/userdata`
//!
//! Release builds deliberately do NOT use a portable dir next to the `.app`: an app
//! installed in /Applications is not writable for standard users, and every process
//! (app, daemon, tray — including a launchd-started daemon) must resolve the same
//! location without environment help.

use std::path::{Path, PathBuf};

pub const DATA_DIR_ENV: &str = "REMOVENT_DATA_DIR";

#[derive(Debug, Clone)]
pub struct DataPaths {
    pub root: PathBuf,
}

impl DataPaths {
    pub fn resolve() -> Self {
        Self {
            root: resolve_root(),
        }
    }

    pub fn identity_dir(&self) -> PathBuf {
        self.root.join("identity")
    }
    pub fn device_key(&self) -> PathBuf {
        self.identity_dir().join("device.key")
    }
    pub fn device_cert(&self) -> PathBuf {
        self.identity_dir().join("device.crt")
    }
    pub fn peers_file(&self) -> PathBuf {
        self.root.join("peers.json")
    }
    pub fn settings_file(&self) -> PathBuf {
        self.root.join("settings.toml")
    }
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }
    pub fn panics_dir(&self) -> PathBuf {
        self.logs_dir().join("panics")
    }
    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }
    pub fn transfers_cache(&self) -> PathBuf {
        self.cache_dir().join("transfers")
    }
    pub fn update_cache(&self) -> PathBuf {
        self.cache_dir().join("update")
    }
    pub fn lock_file(&self) -> PathBuf {
        self.root.join(".lock")
    }
    pub fn run_dir(&self) -> PathBuf {
        self.root.join("run")
    }
    /// daemon IPC socket (architecture.md §8).
    pub fn daemon_socket(&self) -> PathBuf {
        self.run_dir().join("removentd.sock")
    }
    /// daemon-specific mutex lock (separate from the app's .lock).
    pub fn daemon_lock_file(&self) -> PathBuf {
        self.root.join(".daemon.lock")
    }

    pub fn ensure_layout(&self) -> std::io::Result<()> {
        for dir in [
            self.root.clone(),
            self.identity_dir(),
            self.logs_dir(),
            self.panics_dir(),
            self.cache_dir(),
            self.transfers_cache(),
            self.update_cache(),
            self.run_dir(),
        ] {
            std::fs::create_dir_all(dir)?;
        }
        // run/ holds the IPC socket; restrict it to this user.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ =
                std::fs::set_permissions(self.run_dir(), std::fs::Permissions::from_mode(0o700));
        }
        Ok(())
    }
}

fn resolve_root() -> PathBuf {
    match std::env::var_os(DATA_DIR_ENV) {
        Some(dir) if !dir.is_empty() => return PathBuf::from(dir),
        _ => {}
    }
    if cfg!(debug_assertions) && workspace_root().is_some() {
        return workspace_root().unwrap().join("userdata");
    }
    fallback_library_dir()
}

fn workspace_root() -> Option<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest.parent()?.parent().map(|p| p.to_path_buf())
}

fn fallback_library_dir() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join("Library/Application Support/removent/userdata");
    }
    PathBuf::from("userdata")
}

/// Directory writability probe: actually creates and deletes a probe file.
pub fn ensure_writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".write-probe");
    match std::fs::write(&probe, b"ok") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_override_wins() {
        let tmp = tempfile::tempdir().unwrap();
        // SAFETY(env): single-threaded mutation within the test process, restored afterwards.
        unsafe { std::env::set_var(DATA_DIR_ENV, tmp.path()) };
        let p = DataPaths::resolve();
        unsafe { std::env::remove_var(DATA_DIR_ENV) };
        assert_eq!(p.root, tmp.path());
        assert!(ensure_writable(&p.root));
    }

    #[test]
    fn layout_contains_all_subdirs() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths {
            root: tmp.path().to_path_buf(),
        };
        p.ensure_layout().unwrap();
        for d in [
            p.identity_dir(),
            p.logs_dir(),
            p.cache_dir(),
            p.transfers_cache(),
        ] {
            assert!(d.is_dir(), "missing {}", d.display());
        }
    }
}
