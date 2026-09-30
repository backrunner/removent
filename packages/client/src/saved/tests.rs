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
