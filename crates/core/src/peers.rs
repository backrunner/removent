//! Trusted device store (peers.json, protocol.md §4.3).

use crate::error::Result;
use crate::paths::DataPaths;
use removent_proto::Caps;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeerRecord {
    /// Certificate SHA-256 fingerprint as hex (64 chars).
    pub fingerprint: String,
    pub name: String,
    /// Short fingerprint (matches the mDNS TXT fp).
    pub short_fp: String,
    pub granted_caps: Caps,
    /// "This time only" = false: identity is known but not auto-admitted.
    pub trusted: bool,
    pub added_at_unix: u64,
    pub last_connected_unix: u64,
}

#[derive(Debug, Clone, Default)]
pub struct PeersStore {
    peers: Vec<PeerRecord>,
    path: Option<std::path::PathBuf>,
}

impl PeersStore {
    pub fn load(paths: &DataPaths) -> Result<Self> {
        let path = paths.peers_file();
        let peers: Vec<PeerRecord> = match std::fs::read_to_string(&path) {
            Ok(txt) => serde_json::from_str(&txt)?,
            Err(_) => Vec::new(),
        };
        Ok(Self {
            peers,
            path: Some(path),
        })
    }

    /// Diskless store (for engine tests).
    pub fn in_memory() -> Self {
        Self {
            peers: Vec::new(),
            path: None,
        }
    }

    pub fn all(&self) -> &[PeerRecord] {
        &self.peers
    }

    pub fn by_fingerprint(&self, fp_hex: &str) -> Option<&PeerRecord> {
        self.peers.iter().find(|p| p.fingerprint == fp_hex)
    }

    pub fn upsert(&mut self, record: PeerRecord) -> Result<()> {
        match self
            .peers
            .iter_mut()
            .find(|p| p.fingerprint == record.fingerprint)
        {
            Some(existing) => *existing = record,
            None => self.peers.push(record),
        }
        self.flush()
    }

    pub fn remove(&mut self, fp_hex: &str) -> Result<bool> {
        let before = self.peers.len();
        self.peers.retain(|p| p.fingerprint != fp_hex);
        let removed = self.peers.len() != before;
        if removed {
            self.flush()?;
        }
        Ok(removed)
    }

    pub fn touch_connected(&mut self, fp_hex: &str) {
        if let Some(p) = self.peers.iter_mut().find(|p| p.fingerprint == fp_hex) {
            p.last_connected_unix = now_unix();
        }
    }

    pub fn flush(&self) -> Result<()> {
        if let Some(path) = &self.path {
            crate::settings::atomic_write(
                path,
                serde_json::to_vec_pretty(&self.peers)?.as_slice(),
            )?;
        }
        Ok(())
    }
}

pub(crate) fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn rec(fp: &str, trusted: bool) -> PeerRecord {
        PeerRecord {
            fingerprint: fp.into(),
            name: "peer".into(),
            short_fp: fp[..16].into(),
            granted_caps: Caps {
                video: true,
                ..Caps::none()
            },
            trusted,
            added_at_unix: 1,
            last_connected_unix: 2,
        }
    }

    #[test]
    fn upsert_remove_roundtrip() {
        let dir = tempdir().unwrap();
        let p = DataPaths {
            root: dir.path().to_path_buf(),
        };

        let mut store = PeersStore::load(&p).unwrap();
        store.upsert(rec("aa".repeat(32).as_str(), true)).unwrap();
        store.upsert(rec("bb".repeat(32).as_str(), false)).unwrap();

        let mut reloaded = PeersStore::load(&p).unwrap();
        assert_eq!(reloaded.all().len(), 2);
        assert!(reloaded.by_fingerprint(&"aa".repeat(32)).unwrap().trusted);

        assert!(reloaded.remove(&"aa".repeat(32)).unwrap());
        assert!(!reloaded.remove(&"cc".repeat(32)).unwrap());
        assert_eq!(reloaded.all().len(), 1);
    }
}
