use super::*;
#[test]
fn sign_out_keeps_cached_connections_usable_but_owned_by_the_old_account() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(&dir);
    let entry = bookmark(&paths, "Office");
    activate(&paths, "A");
    ack(&paths, "A");
    dispatch(&paths, Command::Suspend).unwrap();
    assert_eq!(
        SavedConnections::load(&paths).unwrap().all()[0].id,
        entry.id
    );
    edit(&paths, &entry.id, |e| e.name = "Offline rename".into());
    assert_eq!(
        dispatch(&paths, Command::Status).unwrap()["account_available"],
        false
    );
    assert!(dispatch(&paths, Command::Snapshot { scope: "A".into() }).is_err());
    dispatch(&paths, Command::Bind { scope: "B".into() }).unwrap();
    assert!(SavedConnections::load(&paths).unwrap().all().is_empty());
    assert!(pending(&paths, "B").is_empty());
    dispatch(&paths, Command::Bind { scope: "A".into() }).unwrap();
    assert_eq!(
        pending(&paths, "A")[0].payload.data.as_ref().unwrap().name,
        "Offline rename"
    );
}
fn paths(dir: &tempfile::TempDir) -> DataPaths {
    DataPaths {
        root: dir.path().into(),
    }
}
fn bookmark(paths: &DataPaths, name: &str) -> SavedConnection {
    SavedConnections::load(paths)
        .unwrap()
        .upsert(SavedConnection {
            name: name.into(),
            host: "office.local".into(),
            port: 5900,
            protocol: ConnectionProtocol::Vnc,
            password_hint: true,
            password_key: Some("local-secret".into()),
            accept_invalid_certificate: true,
            ..Default::default()
        })
        .unwrap()
}
fn activate(paths: &DataPaths, scope: &str) {
    dispatch(paths, Command::Enable { enabled: true }).unwrap();
    dispatch(
        paths,
        Command::Bind {
            scope: scope.into(),
        },
    )
    .unwrap();
}
fn pending(paths: &DataPaths, scope: &str) -> Vec<RemoteRecord> {
    serde_json::from_value(
        dispatch(
            paths,
            Command::Snapshot {
                scope: scope.into(),
            },
        )
        .unwrap()["pending"]
            .clone(),
    )
    .unwrap()
}
fn ack(paths: &DataPaths, scope: &str) {
    dispatch(
        paths,
        Command::Acknowledge {
            scope: scope.into(),
            records: pending(paths, scope),
        },
    )
    .unwrap();
}
fn receive(paths: &DataPaths, scope: &str, records: Vec<RemoteRecord>) {
    dispatch(
        paths,
        Command::Apply {
            scope: scope.into(),
            records,
        },
    )
    .unwrap();
}
fn edit(paths: &DataPaths, id: &str, change: impl FnOnce(&mut SavedConnection)) {
    let mut saved = SavedConnections::load(paths).unwrap();
    let mut entry = saved.all().iter().find(|e| e.id == id).unwrap().clone();
    change(&mut entry);
    saved.upsert(entry).unwrap();
}
#[test]
fn unresolved_conflicts_survive_remote_updates_and_late_acknowledgements() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(&dir);
    let entry = bookmark(&paths, "Before");
    activate(&paths, "A");
    let mut remote = pending(&paths, "A").remove(0);
    ack(&paths, "A");
    edit(&paths, &entry.id, |e| e.name = "Mine".into());
    let in_flight = pending(&paths, "A");
    remote.payload.revision = revision();
    remote.payload.data.as_mut().unwrap().name = "Theirs".into();
    receive(&paths, "A", vec![remote.clone()]);
    remote.payload.revision = revision();
    remote.payload.data.as_mut().unwrap().host = "new.local".into();
    receive(&paths, "A", vec![remote]);
    dispatch(
        &paths,
        Command::Acknowledge {
            scope: "A".into(),
            records: in_flight,
        },
    )
    .unwrap();
    assert!(pending(&paths, "A").is_empty());
    let status = dispatch(&paths, Command::Status).unwrap();
    assert_eq!(status["conflicts"][0]["local"]["name"], "Mine");
    assert_eq!(status["conflicts"][0]["remote"]["host"], "new.local");
    dispatch(
        &paths,
        Command::Resolve {
            id: entry.id,
            keep_local: false,
        },
    )
    .unwrap();
    assert!(pending(&paths, "A").is_empty());
    assert_eq!(
        SavedConnections::load(&paths).unwrap().all()[0].name,
        "Theirs"
    );
}

#[test]
fn metadata_round_trip_excludes_secrets_and_requires_local_trust() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let a = paths(&a);
    let b = paths(&b);
    let entry = bookmark(&a, "Office");
    activate(&a, "user");
    activate(&b, "user");
    let records = pending(&a, "user");
    let encoded = serde_json::to_string(&records).unwrap();
    for secret in [
        "password_key",
        "password_hint",
        "accept_invalid_certificate",
        "local-secret",
        "last_used_unix",
    ] {
        assert!(!encoded.contains(secret));
    }
    receive(&b, "user", records);
    let stored = SavedConnections::load(&b).unwrap();
    assert_eq!(stored.all()[0].id, entry.id);
    assert!(!stored.all()[0].password_hint);
    assert!(stored.all()[0].password_key.is_none());
    assert!(!stored.all()[0].accept_invalid_certificate);
}
#[test]
fn remote_rename_preserves_local_credential_but_scope_change_detaches_it() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(&dir);
    let entry = bookmark(&paths, "Before");
    activate(&paths, "user");
    let mut remote = pending(&paths, "user").remove(0);
    ack(&paths, "user");
    remote.payload.revision = revision();
    remote.payload.data.as_mut().unwrap().name = "After".into();
    receive(&paths, "user", vec![remote.clone()]);
    assert_eq!(
        SavedConnections::load(&paths).unwrap().all()[0]
            .password_key
            .as_deref(),
        Some("local-secret")
    );
    remote.payload.revision = revision();
    remote.payload.data.as_mut().unwrap().host = "another.local".into();
    receive(&paths, "user", vec![remote]);
    let entries = SavedConnections::load(&paths).unwrap();
    assert_eq!(entries.all()[0].id, entry.id);
    assert!(entries.all()[0].password_key.is_none());
    assert!(!entries.all()[0].accept_invalid_certificate);
}
#[test]
fn independent_edits_merge_and_same_field_conflicts_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(&dir);
    let entry = bookmark(&paths, "Before");
    activate(&paths, "user");
    let mut remote = pending(&paths, "user").remove(0);
    ack(&paths, "user");
    edit(&paths, &entry.id, |e| e.name = "Local".into());
    remote.payload.revision = revision();
    remote.payload.data.as_mut().unwrap().host = "remote.local".into();
    receive(&paths, "user", vec![remote]);
    let merged = pending(&paths, "user").remove(0);
    assert_eq!(merged.payload.data.as_ref().unwrap().name, "Local");
    assert_eq!(merged.payload.data.as_ref().unwrap().host, "remote.local");
    ack(&paths, "user");
    edit(&paths, &entry.id, |e| e.name = "Mine".into());
    let mut other = merged;
    other.payload.revision = revision();
    other.payload.data.as_mut().unwrap().name = "Theirs".into();
    receive(&paths, "user", vec![other]);
    assert!(pending(&paths, "user").is_empty());
    assert_eq!(
        dispatch(&paths, Command::Status).unwrap()["conflicts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    dispatch(
        &paths,
        Command::Resolve {
            id: entry.id,
            keep_local: false,
        },
    )
    .unwrap();
    assert_eq!(
        SavedConnections::load(&paths).unwrap().all()[0].name,
        "Theirs"
    );
}
#[test]
fn acknowledgements_do_not_drop_edits_made_during_upload() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(&dir);
    let entry = bookmark(&paths, "First");
    activate(&paths, "user");
    let old = pending(&paths, "user");
    edit(&paths, &entry.id, |e| e.name = "Second".into());
    dispatch(
        &paths,
        Command::Acknowledge {
            scope: "user".into(),
            records: old,
        },
    )
    .unwrap();
    assert_eq!(
        pending(&paths, "user")[0]
            .payload
            .data
            .as_ref()
            .unwrap()
            .name,
        "Second"
    );
}
#[test]
fn offline_delete_is_durable_and_explicit_restore_uses_a_new_id() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(&dir);
    let entry = bookmark(&paths, "Office");
    activate(&paths, "user");
    let mut tombstone = pending(&paths, "user").remove(0);
    ack(&paths, "user");
    edit(&paths, &entry.id, |e| e.name = "Offline edit".into());
    tombstone.payload = Payload::new(None);
    receive(&paths, "user", vec![tombstone]);
    assert!(pending(&paths, "user").is_empty());
    dispatch(
        &paths,
        Command::Resolve {
            id: entry.id.clone(),
            keep_local: true,
        },
    )
    .unwrap();
    let saved = SavedConnections::load(&paths).unwrap();
    assert_ne!(saved.all()[0].id, entry.id);
    assert!(saved.all()[0].password_key.is_none());
    SavedConnections::load(&paths)
        .unwrap()
        .remove(&saved.all()[0].id)
        .unwrap();
    assert!(
        pending(&paths, "user")
            .iter()
            .any(|r| r.payload.data.is_none())
    );
}
#[test]
fn account_switch_isolates_cached_connections_outbox_and_late_callbacks() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(&dir);
    let entry = bookmark(&paths, "Account A");
    activate(&paths, "A");
    let old = pending(&paths, "A");
    dispatch(&paths, Command::Bind { scope: "B".into() }).unwrap();
    assert!(SavedConnections::load(&paths).unwrap().all().is_empty());
    assert!(pending(&paths, "B").is_empty());
    assert!(
        dispatch(
            &paths,
            Command::Apply {
                scope: "A".into(),
                records: old
            }
        )
        .is_err()
    );
    dispatch(&paths, Command::Bind { scope: "A".into() }).unwrap();
    assert_eq!(
        SavedConnections::load(&paths).unwrap().all()[0].id,
        entry.id
    );
    assert_eq!(pending(&paths, "A").len(), 1);
}
#[test]
fn disabled_sync_queues_edits_without_accepting_network_results() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(&dir);
    let entry = bookmark(&paths, "Before");
    activate(&paths, "A");
    ack(&paths, "A");
    dispatch(&paths, Command::Enable { enabled: false }).unwrap();
    edit(&paths, &entry.id, |e| e.name = "Offline".into());
    assert!(dispatch(&paths, Command::Snapshot { scope: "A".into() }).is_err());
    activate(&paths, "A");
    assert_eq!(pending(&paths, "A").len(), 1);
}
#[test]
fn unsupported_or_invalid_record_rolls_back_the_whole_page() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let a = paths(&a);
    let b = paths(&b);
    bookmark(&a, "Office");
    activate(&a, "A");
    activate(&b, "A");
    let mut records = pending(&a, "A");
    let mut bad = records[0].clone();
    bad.id = revision();
    bad.payload.version = 2;
    records.push(bad);
    assert!(
        dispatch(
            &b,
            Command::Apply {
                scope: "A".into(),
                records
            }
        )
        .is_err()
    );
    assert!(SavedConnections::load(&b).unwrap().all().is_empty());
    let mut records = pending(&a, "A");
    records[0].payload.data.as_mut().unwrap().host = "user@host".into();
    assert!(
        dispatch(
            &b,
            Command::Apply {
                scope: "A".into(),
                records
            }
        )
        .is_err()
    );
}
#[test]
fn journal_recovers_a_crash_between_bookmark_and_outbox_writes() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(&dir);
    bookmark(&paths, "Office");
    activate(&paths, "A");
    let saved = SavedConnections::load(&paths).unwrap();
    let journal = Journal {
        version: 1,
        entries: saved.entries.clone(),
        sync: saved.sync.clone(),
    };
    atomic_write(
        &journal_path(&paths.connections_file()),
        &serde_json::to_vec(&journal).unwrap(),
    )
    .unwrap();
    std::fs::write(paths.connections_file(), "[]").unwrap();
    std::fs::remove_file(state_path(&paths.connections_file())).unwrap();
    assert_eq!(SavedConnections::load(&paths).unwrap().all().len(), 1);
    assert_eq!(pending(&paths, "A").len(), 1);
    assert!(!journal_path(&paths.connections_file()).exists());
}
#[test]
fn touch_is_not_a_content_edit_and_token_reset_keeps_outbox() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(&dir);
    let entry = bookmark(&paths, "Office");
    activate(&paths, "A");
    ack(&paths, "A");
    SavedConnections::load(&paths)
        .unwrap()
        .touch(&entry.id)
        .unwrap();
    assert!(pending(&paths, "A").is_empty());
    edit(&paths, &entry.id, |e| e.name = "Pending".into());
    dispatch(
        &paths,
        Command::Checkpoint {
            scope: "A".into(),
            value: "engine-state".into(),
        },
    )
    .unwrap();
    assert_eq!(
        dispatch(&paths, Command::Snapshot { scope: "A".into() }).unwrap()["engine_state"],
        "engine-state"
    );
    dispatch(
        &paths,
        Command::Reset {
            scope: "A".into(),
            zone_deleted: false,
        },
    )
    .unwrap();
    assert!(
        dispatch(&paths, Command::Snapshot { scope: "A".into() }).unwrap()["engine_state"]
            .is_null()
    );
    assert_eq!(
        pending(&paths, "A")[0].payload.data.as_ref().unwrap().name,
        "Pending"
    );
}
