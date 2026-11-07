//! Write-ahead transaction. Recovery derives rename completion from the files,
//! so a power failure between a rename and its journal write is recoverable.
use super::io;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Phase {
    Prepared,
    Installing,
    Verifying,
    Committed,
    RolledBack,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Journal {
    pub phase: Phase,
    pub version: String,
    pub previous: String,
    pub bundle: PathBuf,
    pub incoming: PathBuf,
    pub backup: PathBuf,
    pub resume_host: bool,
    pub resume_tray: bool,
    pub reopen: bool,
    pub baseline: Option<super::health::Health>,
}
impl Journal {
    pub fn new(bundle: &Path, version: &str, previous: &str) -> Self {
        Self {
            phase: Phase::Prepared,
            version: version.into(),
            previous: previous.into(),
            bundle: bundle.into(),
            incoming: bundle.with_extension("app.update-incoming"),
            backup: bundle.with_extension("app.update-backup"),
            resume_host: false,
            resume_tray: false,
            reopen: false,
            baseline: None,
        }
    }
    pub fn validate(&self, bundle: &Path) -> Result<()> {
        let expected = Self::new(bundle, &self.version, &self.previous);
        ensure!(
            self.bundle == expected.bundle
                && self.incoming == expected.incoming
                && self.backup == expected.backup,
            "Update journal points outside this installation"
        );
        for path in [&self.bundle, &self.incoming, &self.backup] {
            if let Ok(meta) = fs::symlink_metadata(path) {
                ensure!(
                    meta.is_dir(),
                    "Update bundle is a symlink or not a directory"
                );
            }
        }
        Ok(())
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        io::save(path, self)
    }
    pub fn publish(&mut self, journal: &Path) -> Result<()> {
        ensure!(
            !self.backup.try_exists()?,
            "Previous backup has not been retired"
        );
        self.phase = Phase::Installing;
        self.save(journal)?;
        fs::rename(&self.bundle, &self.backup)?;
        io::sync_dir(self.bundle.parent().unwrap())?;
        fs::rename(&self.incoming, &self.bundle)?;
        io::sync_dir(self.bundle.parent().unwrap())?;
        self.phase = Phase::Verifying;
        self.save(journal)
    }
    pub fn restore(&mut self, journal: &Path) -> Result<()> {
        // Never delete the only good copy. Interrupted rollback is idempotent.
        if self.backup.try_exists()? {
            if self.bundle.try_exists()? {
                fs::remove_dir_all(&self.bundle)?;
            }
            fs::rename(&self.backup, &self.bundle)?;
            io::sync_dir(self.bundle.parent().unwrap())?;
        }
        ensure!(
            self.bundle.is_dir(),
            "Both installed and rollback bundles are missing"
        );
        self.phase = Phase::RolledBack;
        self.save(journal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_interrupted_rename_restores_the_previous_bundle() {
        for boundary in 0..=3 {
            let dir = tempfile::tempdir().unwrap();
            let bundle = dir.path().join("Removent.app");
            let mut tx = Journal::new(&bundle, "2.0.0", "1.0.0");
            fs::create_dir(&bundle).unwrap();
            fs::write(bundle.join("version"), "old").unwrap();
            fs::create_dir(&tx.incoming).unwrap();
            fs::write(tx.incoming.join("version"), "new").unwrap();
            let log = dir.path().join("journal.json");
            tx.save(&log).unwrap();
            if boundary >= 1 {
                tx.phase = Phase::Installing;
                tx.save(&log).unwrap();
            }
            if boundary >= 2 {
                fs::rename(&bundle, &tx.backup).unwrap();
            }
            if boundary >= 3 {
                fs::rename(&tx.incoming, &bundle).unwrap();
            }
            let mut recovered: Journal = io::load(&log).unwrap();
            recovered.validate(&bundle).unwrap();
            recovered.restore(&log).unwrap();
            recovered.restore(&log).unwrap();
            assert_eq!(fs::read_to_string(bundle.join("version")).unwrap(), "old");
        }
    }
    #[test]
    fn journal_cannot_redirect_rollback() {
        let mut tx = Journal::new(Path::new("/Applications/Removent.app"), "2.0.0", "1.0.0");
        tx.backup = "/Users/someone/Documents".into();
        assert!(
            tx.validate(Path::new("/Applications/Removent.app"))
                .is_err()
        );
    }
}
