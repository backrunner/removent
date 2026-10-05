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
    /// Imported endpoint has not had its credentials reviewed on this device.
    pub credentials_review_required: bool,
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
            credentials_review_required: false,
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
            pairing_code: None,
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
            && (self.password_hint || self.credentials_review_required)
    }
}

#[derive(Debug, Clone, Default)]
pub struct SavedConnections {
    pub(crate) entries: Vec<SavedConnection>,
    pub(crate) sync: crate::cloud_sync::SyncState,
    path: Option<std::path::PathBuf>,
}

impl SavedConnections {
    pub fn load(paths: &DataPaths) -> Result<Self> {
        let path = paths.connections_file();
        let _lock = DataDirLock::acquire_blocking(&path.with_extension("lock"))?;
        crate::cloud_sync::recover(&path)?;
        let entries = crate::cloud_sync::read_entries(&path)?;
        let sync = crate::cloud_sync::read_state(&path)?;
        Ok(Self {
            entries,
            sync,
            path: Some(path),
        })
    }

    /// Diskless store (for tests).
    pub fn in_memory() -> Self {
        Self {
            entries: Vec::new(),
            sync: Default::default(),
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
        self.transaction_inner(true, mutate)
    }

    pub(crate) fn sync_transaction<T>(
        &mut self,
        mutate: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        self.transaction_inner(false, mutate)
    }

    fn transaction_inner<T>(
        &mut self,
        local: bool,
        mutate: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        let _lock = self
            .path
            .as_ref()
            .map(|path| DataDirLock::acquire_blocking(&path.with_extension("lock")))
            .transpose()?;
        let mut next = self.clone();
        if let Some(path) = &self.path {
            crate::cloud_sync::recover(path)?;
            next.entries = crate::cloud_sync::read_entries(path)?;
            next.sync = crate::cloud_sync::read_state(path)?;
        }
        let previous = next.entries.clone();
        let previous_sync = next.sync.clone();
        let result = mutate(&mut next)?;
        if local {
            next.sync.record_local_changes(&previous, &next.entries)?;
        }
        if next.entries != previous || next.sync != previous_sync {
            next.flush()?;
        }
        self.entries = next.entries;
        self.sync = next.sync;
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
                && !self
                    .entries
                    .iter()
                    .any(|e| e.id == entry.id && e.same_endpoint(&entry))
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
            crate::cloud_sync::commit(path, &self.entries, &self.sync)?;
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
mod tests;
