//! Saved connection bookmarks (connections.json).
//!
//! Everything needed to reconnect is persisted — address, protocol, and the
//! non-secret fields — while `password_hint` records whether a secret exists.
//! The password itself lives in the macOS Keychain under a versioned item key
//! (see `keychain`); it never touches this file.

use crate::connection::{ConnectionAddress, ConnectionProtocol, ConnectionRequest, RelayRoute};
use removent_core::{CoreError, DataDirLock, DataPaths, Result};
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
    pub relay: Option<RelayRoute>,
    /// Whether the request carried a password. The secret itself is in the
    /// Keychain under `password_key`; this flag decides whether reconnecting needs a
    /// keychain lookup (and a form fallback when the item is gone).
    pub password_hint: bool,
    /// Versioned Keychain item.
    /// New saves use a fresh item so a failed commit never changes old secrets.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_key: Option<String>,
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
            relay: None,
            password_hint: false,
            password_key: None,
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
            relay: request.relay.clone(),
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
            relay: self.relay.clone(),
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
            && self.relay == other.relay
    }

    /// This entry expects a password: look it up in the Keychain first and fall
    /// back to a prefilled form when the item is missing.
    pub fn credential_account(&self) -> Option<&str> {
        self.password_key
            .as_deref()
            .filter(|_| self.needs_credentials())
    }

    pub fn needs_credentials(&self) -> bool {
        (self.protocol != ConnectionProtocol::Removent || self.relay.is_some())
            && self.password_hint
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

    /// Serialize load/mutate/commit across windows and processes. Atomic rename
    /// alone prevents torn files, but does not prevent lost updates.
    fn transaction<T>(&mut self, mutate: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let _lock = self
            .path
            .as_ref()
            .map(|path| DataDirLock::acquire_blocking(&path.with_extension("lock")))
            .transpose()?;
        let mut next = self.clone();
        if let Some(path) = &self.path {
            next.entries = match std::fs::read(path) {
                Ok(bytes) => serde_json::from_slice(&bytes)?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(e) => return Err(e.into()),
            };
        }
        let result = mutate(&mut next)?;
        next.flush()?;
        self.entries = next.entries;
        Ok(result)
    }

    /// Explicit edits retain identity; new forms deduplicate by endpoint.
    /// Editing to an existing endpoint is rejected rather than merging secrets.
    fn upsert_inner(&mut self, mut entry: SavedConnection) -> Result<SavedConnection> {
        entry.name = entry.name.trim().to_string();
        entry.last_used_unix = now_unix();
        let existing = if entry.id.is_empty() {
            self.entries.iter().position(|e| e.same_endpoint(&entry))
        } else {
            if self
                .entries
                .iter()
                .any(|e| e.id != entry.id && e.same_endpoint(&entry))
            {
                return Err(CoreError::Serialization(
                    "a bookmark for this endpoint already exists".into(),
                ));
            }
            Some(
                self.entries
                    .iter()
                    .position(|e| e.id == entry.id)
                    .ok_or_else(|| {
                        CoreError::Serialization(
                            "this bookmark was removed; create a new connection".into(),
                        )
                    })?,
            )
        };
        if let Some(index) = existing {
            entry.id = self.entries[index].id.clone();
            entry.added_at_unix = self.entries[index].added_at_unix;
            self.entries[index] = entry.clone();
        } else {
            entry.id = format!("{:032x}", rand::random::<u128>());
            entry.added_at_unix = entry.last_used_unix;
            self.entries.push(entry.clone());
        }
        Ok(entry)
    }

    pub fn upsert(&mut self, entry: SavedConnection) -> Result<SavedConnection> {
        self.transaction(|next| next.upsert_inner(entry))
    }

    /// Commit a bookmark and its password reference as one logical operation.
    /// A failed Keychain write or JSON commit leaves the previous bookmark and
    /// its secret intact. Old/orphaned items are never selected by a new save.
    pub fn save_with_password(
        &mut self,
        entry: SavedConnection,
        password: &str,
    ) -> Result<SavedConnection> {
        self.save_with_secret_store(
            entry,
            password,
            crate::keychain::store,
            crate::keychain::delete,
        )
    }

    fn save_with_secret_store(
        &mut self,
        mut entry: SavedConnection,
        password: &str,
        mut put: impl FnMut(&str, &str) -> Result<()>,
        mut delete: impl FnMut(&str) -> Result<()>,
    ) -> Result<SavedConnection> {
        let mut created = None;
        let mut retired = None;
        let result = self.transaction(|next| {
            let previous = next.entries.iter().find(|e| {
                if entry.id.is_empty() {
                    e.same_endpoint(&entry)
                } else {
                    e.id == entry.id
                }
            });
            retired = previous
                .and_then(|e| e.credential_account())
                .map(str::to_owned);
            entry.password_hint = !password.is_empty();
            entry.password_key = entry
                .password_hint
                .then(|| format!("secret-{:032x}", rand::random::<u128>()));
            let stored = next.upsert_inner(entry)?;
            if let Some(account) = stored.credential_account() {
                put(account, password)?;
                created = Some(account.to_owned());
            }
            Ok(stored)
        });
        let cleanup = if result.is_ok() { retired } else { created };
        if let Some(account) = cleanup
            && let Err(error) = delete(&account)
        {
            // No bookmark references this item after the transaction. Report
            // cleanup failure without ever resurrecting it on the next read.
            tracing::warn!(%error, "could not remove retired connection password");
            if result.is_ok() {
                return Err(error);
            }
        }
        result
    }

    pub fn touch(&mut self, id: &str) -> Result<bool> {
        self.transaction(|next| {
            if let Some(entry) = next.entries.iter_mut().find(|e| e.id == id) {
                entry.last_used_unix = now_unix();
                Ok(true)
            } else {
                Ok(false)
            }
        })
    }

    pub fn remove(&mut self, id: &str) -> Result<bool> {
        self.transaction(|next| {
            let before = next.entries.len();
            next.entries.retain(|e| e.id != id);
            Ok(next.entries.len() != before)
        })
    }

    pub fn remove_with_password(&mut self, id: &str) -> Result<()> {
        let account = self.transaction(|next| {
            let account = next
                .entries
                .iter()
                .find(|e| e.id == id)
                .and_then(|e| e.credential_account())
                .map(str::to_owned);
            next.entries.retain(|e| e.id != id);
            Ok(account)
        })?;
        if let Some(account) = account {
            crate::keychain::delete(&account)?;
        }
        Ok(())
    }

    fn flush(&self) -> Result<()> {
        if let Some(path) = &self.path {
            removent_core::settings::atomic_write(
                path,
                &serde_json::to_vec_pretty(&self.entries)?,
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
            relay: None,
        }
    }

    #[test]
    fn relay_bookmarks_roundtrip_trust_and_keep_credentials_in_secret_store() {
        let dir = tempdir().unwrap();
        let paths = DataPaths {
            root: dir.path().to_owned(),
        };
        paths.ensure_layout().unwrap();
        let mut store = SavedConnections::load(&paths).unwrap();
        let mut req = request(ConnectionProtocol::Removent, "office", 0);
        req.relay = Some(
            RelayRoute::parse(
                "removent://relay.example:443",
                crate::connection::RelayTransport::WebSocket,
                "",
                &"aa".repeat(32),
            )
            .unwrap(),
        );
        let mut secrets = std::collections::HashMap::new();
        let saved = store
            .save_with_secret_store(
                SavedConnection::from_request(&req, "Office"),
                &req.password,
                |key, secret| {
                    secrets.insert(key.to_owned(), secret.to_owned());
                    Ok(())
                },
                |_| Ok(()),
            )
            .unwrap();
        assert!(saved.needs_credentials());
        assert_eq!(
            secrets.get(saved.credential_account().unwrap()),
            Some(&req.password)
        );
        let reloaded = SavedConnections::load(&paths).unwrap();
        let restored = reloaded.all()[0].to_request();
        assert_eq!(restored.relay, req.relay);
        assert!(restored.password.is_empty());
        assert!(
            !std::fs::read_to_string(paths.connections_file())
                .unwrap()
                .contains(&req.password)
        );
        req.relay.as_mut().unwrap().endpoint = "removent://second.example:443".into();
        store
            .upsert(SavedConnection::from_request(&req, "Second relay"))
            .unwrap();
        assert_eq!(
            store.all().len(),
            2,
            "same room on different relays must not share a bookmark or secret"
        );
        let mut cleared = saved;
        cleared.relay = None;
        let cleared = store
            .save_with_secret_store(
                cleared,
                "",
                |_, _| panic!("no secret expected"),
                |key| {
                    secrets.remove(key);
                    Ok(())
                },
            )
            .unwrap();
        assert!(!cleared.needs_credentials());
        assert!(secrets.is_empty());
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
    fn stale_windows_preserve_edits_deletes_and_other_inserts() {
        let dir = tempdir().unwrap();
        let paths = DataPaths {
            root: dir.path().to_owned(),
        };
        let mut a = SavedConnections::load(&paths).unwrap();
        let mut b = SavedConnections::load(&paths).unwrap();
        let original = a
            .upsert(SavedConnection::from_request(
                &request(ConnectionProtocol::Vnc, "old.local", 5900),
                "Original",
            ))
            .unwrap();
        b.upsert(SavedConnection::from_request(
            &request(ConnectionProtocol::Vnc, "other.local", 5900),
            "Other",
        ))
        .unwrap();
        assert_eq!(b.all().len(), 2);
        let mut edit = original.clone();
        edit.host = "new.local".into();
        edit.name = "Edited".into();
        a.upsert(edit).unwrap();
        b.touch(&original.id).unwrap();
        assert_eq!(b.all()[0].host, "new.local");
        assert_eq!(b.all()[0].name, "Edited");
        assert_eq!(b.all().len(), 2);
        a.remove(&original.id).unwrap();
        assert!(!b.touch(&original.id).unwrap());
        assert!(b.upsert(original).is_err());
        assert_eq!(SavedConnections::load(&paths).unwrap().all().len(), 1);
    }

    #[test]
    fn concurrent_bookmark_transactions_keep_every_insert() {
        let dir = tempdir().unwrap();
        let paths = DataPaths {
            root: dir.path().to_owned(),
        };
        std::thread::scope(|scope| {
            for worker in 0..8 {
                let paths = paths.clone();
                scope.spawn(move || {
                    let mut store = SavedConnections::load(&paths).unwrap();
                    for index in 0..10 {
                        store
                            .upsert(SavedConnection::from_request(
                                &request(
                                    ConnectionProtocol::Vnc,
                                    &format!("{worker}-{index}.local"),
                                    5900,
                                ),
                                "",
                            ))
                            .unwrap();
                    }
                });
            }
        });
        assert_eq!(SavedConnections::load(&paths).unwrap().all().len(), 80);
    }

    #[test]
    fn failed_secret_write_preserves_bookmark_and_clear_never_reuses_old_secret() {
        let mut store = SavedConnections::in_memory();
        let original = store
            .upsert(SavedConnection::from_request(
                &request(ConnectionProtocol::Vnc, "old.local", 5900),
                "Old",
            ))
            .unwrap();
        let mut edit = original.clone();
        edit.name = "New".into();
        assert!(
            store
                .save_with_secret_store(
                    edit.clone(),
                    "new password",
                    |_, _| Err(CoreError::SecureStore("locked".into())),
                    |_| Ok(())
                )
                .is_err()
        );
        assert_eq!(store.all(), std::slice::from_ref(&original));
        let updated = store
            .save_with_secret_store(
                edit.clone(),
                "new password",
                |account, _| {
                    assert_ne!(account, original.id);
                    Ok(())
                },
                |_| Ok(()),
            )
            .unwrap();
        assert_ne!(updated.credential_account(), original.credential_account());
        // Deletion may fail, but after clearing no lookup can resurrect the item.
        assert!(
            store
                .save_with_secret_store(
                    edit,
                    "",
                    |_, _| panic!("must not store empty password"),
                    |_| Err(CoreError::SecureStore("locked".into()))
                )
                .is_err()
        );
        assert!(store.all()[0].credential_account().is_none());
        assert!(!store.all()[0].password_hint);
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
