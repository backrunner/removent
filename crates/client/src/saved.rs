//! Saved connection bookmarks (connections.json).
//!
//! Everything needed to reconnect is persisted — address, protocol, and the
//! non-secret fields — except the password, which stays per-connection and is
//! never written to disk (mirrors the `ConnectionRequest` contract).

use crate::connection::{ConnectionAddress, ConnectionProtocol, ConnectionRequest};
use removent_core::{DataPaths, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedConnection {
    /// Stable identity for sidebar selection and deletion; assigned on insert.
    pub id: String,
    /// User memo name; display falls back to the address when empty.
    pub name: String,
    pub protocol: ConnectionProtocol,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub domain: String,
    pub accept_invalid_certificate: bool,
    /// Records only whether the request carried a password (never the password
    /// itself): lets the UI reconnect directly to passwordless VNC/RDP servers
    /// while still prompting for credentials where one was used.
    pub password_hint: bool,
    pub added_at_unix: u64,
    pub last_used_unix: u64,
}

impl Default for SavedConnection {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            protocol: ConnectionProtocol::Removent,
            host: String::new(),
            port: 0,
            username: String::new(),
            domain: String::new(),
            accept_invalid_certificate: false,
            password_hint: false,
            added_at_unix: 0,
            last_used_unix: 0,
        }
    }
}

impl SavedConnection {
    /// Identity/timestamps are placeholders: `SavedConnections::upsert` assigns them.
    pub fn from_request(request: &ConnectionRequest, name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            protocol: request.protocol,
            host: request.address.host.clone(),
            port: request.address.port,
            username: request.username.clone(),
            domain: request.domain.clone(),
            accept_invalid_certificate: request.accept_invalid_certificate,
            password_hint: !request.password.is_empty(),
            ..Self::default()
        }
    }

    /// Rebuild a request; the password field is empty because it is never persisted.
    pub fn to_request(&self) -> ConnectionRequest {
        ConnectionRequest {
            protocol: self.protocol,
            address: ConnectionAddress {
                host: self.host.clone(),
                port: self.port,
            },
            username: self.username.clone(),
            password: String::new(),
            domain: self.domain.clone(),
            accept_invalid_certificate: self.accept_invalid_certificate,
        }
    }

    pub fn address(&self) -> ConnectionAddress {
        ConnectionAddress {
            host: self.host.clone(),
            port: self.port,
        }
    }

    /// Memo name when set, otherwise the endpoint.
    pub fn display_name(&self) -> String {
        if self.name.is_empty() {
            self.address().to_string()
        } else {
            self.name.clone()
        }
    }

    /// Two entries are the same bookmark when they target the same endpoint with
    /// the same credentials fields (a re-save updates the memo name in place).
    fn same_endpoint(&self, other: &Self) -> bool {
        self.protocol == other.protocol
            && self.host == other.host
            && self.port == other.port
            && self.username == other.username
            && self.domain == other.domain
    }

    /// Removent reconnects through trust/PIN; VNC/RDP need a re-opened form only
    /// when the saved session originally used a password.
    pub fn needs_credentials(&self) -> bool {
        self.protocol != ConnectionProtocol::Removent && self.password_hint
    }
}

#[derive(Debug, Clone, Default)]
pub struct SavedConnections {
    entries: Vec<SavedConnection>,
    path: Option<std::path::PathBuf>,
}

impl SavedConnections {
    pub fn load(paths: &DataPaths) -> Result<Self> {
        let path = paths.connections_file();
        let entries: Vec<SavedConnection> = match std::fs::read_to_string(&path) {
            Ok(txt) => serde_json::from_str(&txt)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        Ok(Self {
            entries,
            path: Some(path),
        })
    }

    /// Diskless store (for tests).
    pub fn in_memory() -> Self {
        Self {
            entries: Vec::new(),
            path: None,
        }
    }

    /// Insertion order is the display order.
    pub fn all(&self) -> &[SavedConnection] {
        &self.entries
    }

    /// Insert a new bookmark, or refresh the memo name/timestamp of the matching
    /// endpoint. The write is committed before the in-memory list changes, so a
    /// failed save never mutates the store.
    pub fn upsert(&mut self, mut entry: SavedConnection) -> Result<()> {
        entry.name = entry.name.trim().to_string();
        entry.last_used_unix = now_unix();
        let mut next = self.clone();
        if let Some(existing) = next.entries.iter_mut().find(|e| e.same_endpoint(&entry)) {
            entry.id = existing.id.clone();
            entry.added_at_unix = existing.added_at_unix;
            *existing = entry;
        } else {
            if entry.id.is_empty() {
                entry.id = format!("{:016x}", rand::random::<u64>());
            }
            entry.added_at_unix = entry.last_used_unix;
            next.entries.push(entry);
        }
        next.flush()?;
        self.entries = next.entries;
        Ok(())
    }

    pub fn remove(&mut self, id: &str) -> Result<bool> {
        let before = self.entries.len();
        let mut next = self.clone();
        next.entries.retain(|e| e.id != id);
        let removed = next.entries.len() != before;
        if removed {
            next.flush()?;
            self.entries = next.entries;
        }
        Ok(removed)
    }

    pub fn flush(&self) -> Result<()> {
        if let Some(path) = &self.path {
            removent_core::settings::atomic_write(
                path,
                serde_json::to_vec_pretty(&self.entries)?.as_slice(),
            )?;
        }
        Ok(())
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn request(protocol: ConnectionProtocol, host: &str, port: u16) -> ConnectionRequest {
        ConnectionRequest {
            protocol,
            address: ConnectionAddress {
                host: host.into(),
                port,
            },
            username: String::new(),
            password: "never-persisted".into(),
            domain: String::new(),
            accept_invalid_certificate: false,
        }
    }

    #[test]
    fn upsert_dedupes_by_endpoint_and_keeps_id() {
        let dir = tempdir().unwrap();
        let paths = DataPaths {
            root: dir.path().to_owned(),
        };
        paths.ensure_layout().unwrap();
        let mut store = SavedConnections::load(&paths).unwrap();
        store
            .upsert(SavedConnection::from_request(
                &request(ConnectionProtocol::Vnc, "office.local", 5900),
                "Office",
            ))
            .unwrap();
        let first_id = store.all()[0].id.clone();
        store
            .upsert(SavedConnection::from_request(
                &request(ConnectionProtocol::Vnc, "office.local", 5900),
                "Office Mac",
            ))
            .unwrap();
        store
            .upsert(SavedConnection::from_request(
                &request(ConnectionProtocol::Rdp, "win.local", 3389),
                "",
            ))
            .unwrap();

        let reloaded = SavedConnections::load(&paths).unwrap();
        assert_eq!(reloaded.all().len(), 2);
        assert_eq!(reloaded.all()[0].id, first_id);
        assert_eq!(reloaded.all()[0].name, "Office Mac");
        assert_eq!(reloaded.all()[0].display_name(), "Office Mac");
        assert_eq!(reloaded.all()[1].display_name(), "win.local:3389");
        // Passwords are never persisted, even though the request carried one;
        // only the fact that a password was used is retained.
        assert!(
            !std::fs::read_to_string(paths.connections_file())
                .unwrap()
                .contains("never-persisted")
        );
        assert!(reloaded.all()[0].to_request().password.is_empty());
        assert!(reloaded.all()[0].password_hint);
        assert!(reloaded.all()[0].needs_credentials());
        // A passwordless entry reconnects without reopening the form.
        let mut no_password = request(ConnectionProtocol::Vnc, "open.local", 5900);
        no_password.password.clear();
        store
            .upsert(SavedConnection::from_request(&no_password, ""))
            .unwrap();
        assert!(!store.all()[2].needs_credentials());
    }

    #[test]
    fn different_credentials_are_distinct_bookmarks() {
        let mut store = SavedConnections::in_memory();
        let mut r = request(ConnectionProtocol::Rdp, "win.local", 3389);
        store
            .upsert(SavedConnection::from_request(&r, "work"))
            .unwrap();
        r.username = "alice".into();
        store
            .upsert(SavedConnection::from_request(&r, "work (alice)"))
            .unwrap();
        assert_eq!(store.all().len(), 2);
    }

    #[test]
    fn failed_save_does_not_change_in_memory_list() {
        let dir = tempdir().unwrap();
        let paths = DataPaths {
            root: dir.path().to_owned(),
        };
        paths.ensure_layout().unwrap();
        let mut store = SavedConnections::load(&paths).unwrap();
        store
            .upsert(SavedConnection::from_request(
                &request(ConnectionProtocol::Vnc, "a.local", 5900),
                "a",
            ))
            .unwrap();
        let id = store.all()[0].id.clone();
        std::fs::remove_file(paths.connections_file()).unwrap();
        std::fs::create_dir(paths.connections_file()).unwrap();
        assert!(
            store
                .upsert(SavedConnection::from_request(
                    &request(ConnectionProtocol::Vnc, "b.local", 5900),
                    "b",
                ))
                .is_err()
        );
        assert!(store.remove(&id).is_err());
        assert_eq!(store.all().len(), 1);
    }
}
