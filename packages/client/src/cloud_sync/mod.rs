//! Account-scoped connection sync. CloudKit owns transport; Rust owns data policy.
//!
//! A durable redo journal commits bookmarks and the outbox together. No password,
//! device identity or certificate exception is included in the wire representation.

use crate::{
    connection::{ConnectionAddress, ConnectionProtocol, RelayRoute},
    saved::{SavedConnection, SavedConnections},
};
use removent_core::{CoreError, DataPaths, Result, settings::atomic_write};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

fn failure(message: impl Into<String>) -> CoreError {
    CoreError::Serialization(message.into())
}
fn revision() -> String {
    format!("{:032x}", rand::random::<u128>())
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionData {
    pub name: String,
    pub protocol: ConnectionProtocol,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub domain: String,
    pub relay: Option<RelayRoute>,
}

impl From<&SavedConnection> for ConnectionData {
    fn from(entry: &SavedConnection) -> Self {
        Self {
            name: entry.name.clone(),
            protocol: entry.protocol,
            host: entry.host.clone(),
            port: entry.port,
            username: entry.username.clone(),
            domain: entry.domain.clone(),
            relay: entry.relay.clone().map(|mut route| {
                route.accept_invalid_certificate = false;
                route
            }),
        }
    }
}

impl ConnectionData {
    fn validate(&self) -> Result<()> {
        if [&self.name, &self.host, &self.username, &self.domain]
            .iter()
            .any(|s| s.len() > 4096 || s.contains('\0'))
        {
            return Err(failure("Invalid cloud connection text"));
        }
        let address = if let Some(route) = &self.relay {
            if self.protocol != ConnectionProtocol::Removent || self.port != 0 {
                return Err(failure("Invalid relay protocol"));
            }
            let parsed = route.validated(false).map_err(failure)?;
            if route.accept_invalid_certificate || &parsed != route {
                return Err(failure("Noncanonical relay route"));
            }
            ConnectionAddress::relay_room(&self.host)
        } else {
            ConnectionAddress::parse(&self.host, &self.port.to_string())
        }
        .map_err(|e| failure(e.to_string()))?;
        if address.host != self.host || address.port != self.port {
            return Err(failure("Noncanonical cloud address"));
        }
        if self.protocol == ConnectionProtocol::Rdp && self.username.trim().is_empty() {
            return Err(failure("RDP requires a username"));
        }
        Ok(())
    }
    fn same_scope(&self, other: &Self) -> bool {
        self.protocol == other.protocol
            && self.host == other.host
            && self.port == other.port
            && self.username == other.username
            && self.domain == other.domain
            && self.relay == other.relay
    }
    fn to_local(&self, id: &str, previous: Option<&SavedConnection>) -> SavedConnection {
        let mut entry = SavedConnection {
            id: id.into(),
            name: self.name.clone(),
            protocol: self.protocol,
            host: self.host.clone(),
            port: self.port,
            username: self.username.clone(),
            domain: self.domain.clone(),
            relay: self.relay.clone(),
            added_at_unix: now(),
            credentials_review_required: self.protocol != ConnectionProtocol::Removent
                || self.relay.is_some(),
            ..Default::default()
        };
        if let Some(previous) = previous {
            entry.added_at_unix = previous.added_at_unix;
            entry.last_used_unix = previous.last_used_unix;
            if self.same_scope(&Self::from(previous)) {
                entry.password_hint = previous.password_hint;
                entry.credentials_review_required = previous.credentials_review_required;
                entry.password_key = previous.password_key.clone();
                entry.accept_invalid_certificate = previous.accept_invalid_certificate;
                if let (Some(route), Some(old)) = (&mut entry.relay, &previous.relay) {
                    route.accept_invalid_certificate = old.accept_invalid_certificate;
                }
            }
        }
        entry
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Payload {
    pub version: u32,
    pub revision: String,
    pub data: Option<ConnectionData>,
}
impl Payload {
    fn new(data: Option<ConnectionData>) -> Self {
        Self {
            version: 1,
            revision: revision(),
            data,
        }
    }
    fn validate(&self) -> Result<()> {
        if self.version != 1 {
            return Err(failure(
                "Unsupported cloud schema; update Removent before syncing",
            ));
        }
        valid_id(&self.revision)?;
        if let Some(data) = &self.data {
            data.validate()?;
        }
        Ok(())
    }
}
fn valid_id(id: &str) -> Result<()> {
    if id.len() == 32 && id.bytes().all(|c| c.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(failure("Invalid cloud record ID"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Record {
    local: Payload,
    base: Option<Payload>,
    #[serde(default)]
    system_fields: Option<String>,
    #[serde(default)]
    conflict: Option<Payload>,
}
impl Record {
    fn pending(&self) -> bool {
        self.base.as_ref() != Some(&self.local) && self.conflict.is_none()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Profile {
    records: BTreeMap<String, Record>,
    archived: Vec<SavedConnection>,
    engine_state: Option<String>,
    last_success: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct SyncState {
    version: u32,
    enabled: bool,
    active: Option<String>,
    suspended: bool,
    profiles: BTreeMap<String, Profile>,
    code: String,
    retry: u64,
}
impl Default for SyncState {
    fn default() -> Self {
        Self {
            version: 1,
            enabled: false,
            active: None,
            suspended: false,
            profiles: BTreeMap::new(),
            code: "off".into(),
            retry: 0,
        }
    }
}

impl SyncState {
    pub(crate) fn record_local_changes(
        &mut self,
        before: &[SavedConnection],
        after: &[SavedConnection],
    ) -> Result<()> {
        let Some(scope) = &self.active else {
            return Ok(());
        };
        let profile = self
            .profiles
            .get_mut(scope)
            .ok_or_else(|| failure("Missing sync profile"))?;
        for entry in after {
            let data = ConnectionData::from(entry);
            if before
                .iter()
                .find(|e| e.id == entry.id)
                .map(ConnectionData::from)
                .as_ref()
                == Some(&data)
            {
                continue;
            }
            valid_id(&entry.id)?;
            data.validate()?;
            match profile.records.get_mut(&entry.id) {
                Some(record) => record.local = Payload::new(Some(data)),
                None => {
                    profile.records.insert(
                        entry.id.clone(),
                        Record {
                            local: Payload::new(Some(data)),
                            base: None,
                            system_fields: None,
                            conflict: None,
                        },
                    );
                }
            }
        }
        for old in before
            .iter()
            .filter(|e| !after.iter().any(|a| a.id == e.id))
        {
            if let Some(record) = profile.records.get_mut(&old.id) {
                record.local = Payload::new(None);
            }
        }
        Ok(())
    }
    fn profile_mut(&mut self, scope: &str) -> Result<&mut Profile> {
        if !self.enabled || self.suspended || self.active.as_deref() != Some(scope) {
            return Err(failure("Sync account changed or sync is disabled"));
        }
        self.profiles
            .get_mut(scope)
            .ok_or_else(|| failure("Missing sync profile"))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoteRecord {
    pub id: String,
    pub payload: Payload,
    pub system_fields: Option<String>,
    #[serde(default)]
    pub base_revision: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Status,
    Enable {
        enabled: bool,
    },
    Retry,
    Bind {
        scope: String,
    },
    Suspend,
    Snapshot {
        scope: String,
    },
    Apply {
        scope: String,
        records: Vec<RemoteRecord>,
    },
    Removed {
        scope: String,
        ids: Vec<String>,
    },
    Acknowledge {
        scope: String,
        records: Vec<RemoteRecord>,
    },
    Checkpoint {
        scope: String,
        value: String,
    },
    Reset {
        scope: String,
        #[serde(default)]
        zone_deleted: bool,
    },
    Report {
        scope: Option<String>,
        code: String,
    },
    Resolve {
        id: String,
        keep_local: bool,
    },
}

pub fn dispatch(paths: &DataPaths, command: Command) -> Result<Value> {
    let mut store = SavedConnections::load(paths)?;
    match command {
        Command::Status => return Ok(status(&store.sync)),
        Command::Snapshot { scope } => {
            let profile = store.sync.profile_mut(&scope)?;
            return Ok(json!({"engine_state": profile.engine_state,
                "pending": profile.records.iter().filter(|(_, r)| r.pending()).map(|(id, r)|
                    RemoteRecord { id: id.clone(), payload: r.local.clone(), system_fields: r.system_fields.clone(), base_revision: r.base.as_ref().map(|p| p.revision.clone()) }).collect::<Vec<_>>() }));
        }
        _ => {}
    }
    store.sync_transaction(|store| {
        match command {
            Command::Enable { enabled } => {
                store.sync.enabled = enabled;
                store.sync.code = if enabled { "waiting" } else { "off" }.into();
            }
            Command::Retry => {
                store.sync.retry = store.sync.retry.wrapping_add(1);
            }
            Command::Bind { scope } => bind(store, scope)?,
            Command::Suspend => {
                // Keep the current account's cached connections usable offline.
                // Their edits remain owned by that account, never by a later login.
                store.sync.suspended = true;
                store.sync.code = "account_unavailable".into();
            }
            Command::Apply { scope, records } => {
                for record in &records {
                    valid_id(&record.id)?;
                    record.payload.validate()?;
                }
                for record in records {
                    apply(store, &scope, record, false)?;
                }
            }
            Command::Removed { scope, ids } => {
                for id in ids {
                    valid_id(&id)?;
                    apply(
                        store,
                        &scope,
                        RemoteRecord {
                            id: id.clone(),
                            payload: Payload::new(None),
                            system_fields: None,
                            base_revision: None,
                        },
                        false,
                    )?;
                    // Convert physical server deletions into durable logical tombstones.
                    store
                        .sync
                        .profile_mut(&scope)?
                        .records
                        .get_mut(&id)
                        .unwrap()
                        .base = None;
                }
            }
            Command::Acknowledge { scope, records } => {
                for record in &records {
                    valid_id(&record.id)?;
                    record.payload.validate()?;
                }
                for record in records {
                    apply(store, &scope, record, true)?;
                }
            }
            Command::Checkpoint { scope, value } => {
                if value.len() > 4 * 1024 * 1024 {
                    return Err(failure("Invalid sync checkpoint"));
                }
                store.sync.profile_mut(&scope)?.engine_state = Some(value);
            }
            Command::Reset {
                scope,
                zone_deleted,
            } => {
                let profile = store.sync.profile_mut(&scope)?;
                profile.engine_state = None;
                // Keep local content and deletion tombstones when a zone disappears.
                if zone_deleted {
                    for record in profile.records.values_mut() {
                        record.base = None;
                        record.system_fields = None;
                    }
                }
            }
            Command::Report { scope, code } => {
                if !matches!(
                    code.as_str(),
                    "waiting"
                        | "syncing"
                        | "ready"
                        | "offline"
                        | "quota"
                        | "configuration"
                        | "error"
                        | "account_unavailable"
                        | "upgrade_required"
                ) {
                    return Err(failure("Invalid sync status"));
                }
                if let Some(scope) = scope {
                    let profile = store.sync.profile_mut(&scope)?;
                    if code == "ready" {
                        profile.last_success = now();
                    }
                } else if !store.sync.enabled {
                    return Ok(status(&store.sync));
                }
                store.sync.code = code;
            }
            Command::Resolve { id, keep_local } => resolve(store, &id, keep_local)?,
            Command::Status | Command::Snapshot { .. } => unreachable!(),
        }
        Ok(status(&store.sync))
    })
}

fn status(state: &SyncState) -> Value {
    let profile = state.active.as_ref().and_then(|s| state.profiles.get(s));
    let conflicts: Vec<_> = profile.into_iter().flat_map(|p| p.records.iter()).filter(|(_, r)| r.conflict.is_some()).map(|(id, r)| {
        json!({"id":id, "local":r.local.data, "remote":r.conflict.as_ref().and_then(|p| p.data.clone())})
    }).collect();
    json!({"enabled":state.enabled, "account_available":state.active.is_some() && !state.suspended,
        "code":if !state.enabled { "off" } else if !conflicts.is_empty() { "conflict" } else { &state.code },
        "pending":profile.map_or(0, |p| p.records.values().filter(|r| r.pending()).count()),
        "conflicts":conflicts, "last_success":profile.map_or(0, |p| p.last_success), "retry":state.retry})
}

mod merge;
mod storage;
#[cfg(test)]
mod tests;

use merge::{Journal, apply, bind, resolve};
pub(crate) use storage::commit;
pub(crate) use storage::read_entries;
pub(crate) use storage::read_state;
pub(crate) use storage::recover;
#[cfg(test)]
use storage::{journal_path, state_path};
