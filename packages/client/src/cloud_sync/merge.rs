use super::*;

pub(super) fn detach(store: &mut SavedConnections) {
    if let Some(scope) = store.sync.active.take()
        && let Some(profile) = store.sync.profiles.get_mut(&scope)
    {
        profile.archived = store
            .entries
            .iter()
            .filter(|e| profile.records.contains_key(&e.id))
            .cloned()
            .collect();
        store
            .entries
            .retain(|e| !profile.records.contains_key(&e.id));
    }
}
pub(super) fn bind(store: &mut SavedConnections, scope: String) -> Result<()> {
    if !store.sync.enabled {
        return Err(failure("Sync is disabled"));
    }
    if scope.is_empty() || scope.len() > 512 || scope.chars().any(char::is_control) {
        return Err(failure("Invalid account scope"));
    }
    if store.sync.active.as_deref() == Some(&scope) {
        store.sync.suspended = false;
        return Ok(());
    }
    detach(store);
    let profile = store.sync.profiles.entry(scope.clone()).or_default();
    // Unsynced connections are enrolled only once; another account's data is archived.
    for entry in &mut store.entries {
        if profile.records.contains_key(&entry.id) {
            entry.id = revision();
        }
        valid_id(&entry.id)?;
        let data = ConnectionData::from(&*entry);
        data.validate()?;
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
    store.entries.append(&mut profile.archived);
    store.sync.active = Some(scope);
    store.sync.suspended = false;
    store.sync.code = "waiting".into();
    Ok(())
}

pub(super) fn install(entries: &mut Vec<SavedConnection>, id: &str, payload: &Payload) {
    let previous = entries.iter().find(|e| e.id == id);
    let next = payload.data.as_ref().map(|d| d.to_local(id, previous));
    if let Some(index) = entries.iter().position(|e| e.id == id) {
        match next {
            Some(entry) => entries[index] = entry,
            None => {
                entries.remove(index);
            }
        }
    } else if let Some(entry) = next {
        entries.push(entry);
    }
}

// Merge independent fields, treating the complete credential scope as one field.
pub(super) fn merge(
    base: &ConnectionData,
    local: &ConnectionData,
    remote: &ConnectionData,
) -> Option<ConnectionData> {
    let name = if local.name == base.name {
        &remote.name
    } else if remote.name == base.name || local.name == remote.name {
        &local.name
    } else {
        return None;
    };
    let mut result = if local.same_scope(base) {
        remote.clone()
    } else if remote.same_scope(base) || local.same_scope(remote) {
        local.clone()
    } else {
        return None;
    };
    result.name = name.clone();
    Some(result)
}

pub(super) fn apply(
    store: &mut SavedConnections,
    scope: &str,
    remote: RemoteRecord,
    acknowledged: bool,
) -> Result<()> {
    let profile = store.sync.profile_mut(scope)?;
    let RemoteRecord {
        id,
        payload,
        system_fields,
        base_revision,
    } = remote;
    if !profile.records.contains_key(&id) && !acknowledged {
        install(&mut store.entries, &id, &payload);
    }
    let record = profile.records.entry(id.clone()).or_insert_with(|| Record {
        local: payload.clone(),
        base: Some(payload.clone()),
        system_fields: system_fields.clone(),
        conflict: None,
    });
    if acknowledged {
        // A send completion must not discard an edit made while that send was in flight.
        let current_base = record.base.as_ref().map(|p| p.revision.as_str());
        if current_base == base_revision.as_deref()
            || current_base == Some(payload.revision.as_str())
        {
            record.base = Some(payload);
            record.system_fields = system_fields;
        }
        return Ok(());
    }
    if record.base.as_ref() == Some(&payload) {
        record.system_fields = system_fields;
        return Ok(());
    }
    if record.base.as_ref() == Some(&record.local) || record.local.data == payload.data {
        record.local = payload.clone();
        record.conflict = None;
    } else if record.conflict.is_some() {
        record.conflict = Some(payload.clone());
    } else if let (Some(base), Some(local), Some(other)) = (
        record.base.as_ref().and_then(|p| p.data.as_ref()),
        record.local.data.as_ref(),
        payload.data.as_ref(),
    ) {
        if let Some(merged) = merge(base, local, other) {
            record.local = if merged == *other {
                payload.clone()
            } else {
                Payload::new(Some(merged))
            };
            record.conflict = None;
        } else {
            record.conflict = Some(payload.clone());
        }
    } else {
        record.conflict = Some(payload.clone());
    }
    record.base = Some(payload);
    record.system_fields = system_fields;
    // Keep a conflict's local value visible until the user resolves it.
    install(&mut store.entries, &id, &record.local);
    Ok(())
}

pub(super) fn resolve(store: &mut SavedConnections, id: &str, keep_local: bool) -> Result<()> {
    let scope = store
        .sync
        .active
        .clone()
        .ok_or_else(|| failure("No sync account"))?;
    let profile = store.sync.profile_mut(&scope)?;
    let record = profile
        .records
        .get_mut(id)
        .ok_or_else(|| failure("Connection no longer exists"))?;
    let remote = record
        .conflict
        .take()
        .ok_or_else(|| failure("Conflict already resolved"))?;
    if keep_local && remote.data.is_none() && record.local.data.is_some() {
        let data = record.local.data.clone();
        record.local = remote.clone();
        let new_id = revision();
        let recreated = Payload::new(data);
        // Re-creating a deleted record uses a new identity and requires credentials again.
        profile.records.insert(
            new_id.clone(),
            Record {
                local: recreated.clone(),
                base: None,
                system_fields: None,
                conflict: None,
            },
        );
        install(&mut store.entries, id, &remote);
        install(&mut store.entries, &new_id, &recreated);
    } else {
        record.local = if keep_local {
            Payload::new(record.local.data.clone())
        } else {
            remote
        };
        install(&mut store.entries, id, &record.local);
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
pub(super) struct Journal {
    pub(super) version: u32,
    pub(super) entries: Vec<SavedConnection>,
    pub(super) sync: SyncState,
}
