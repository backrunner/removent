use super::*;
use tempfile::tempdir;

#[test]
fn wake_settings_roundtrip_preserve_credentials_and_concurrent_edits() {
    let dir = tempdir().unwrap();
    let paths = DataPaths {
        root: dir.path().into(),
    };
    let mut store = SavedConnections::load(&paths).unwrap();
    let config =
        crate::wake_on_lan::WakeOnLan::parse("021122334455", "192.168.1.255", "9").unwrap();
    let entry = store
        .upsert(SavedConnection {
            host: "office.local".into(),
            port: 5900,
            protocol: ConnectionProtocol::Vnc,
            password_hint: true,
            password_key: Some("existing-secret".into()),
            ..Default::default()
        })
        .unwrap();
    let mut other = SavedConnections::load(&paths).unwrap();
    let mut renamed = entry.clone();
    renamed.name = "New name".into();
    other.upsert(renamed).unwrap();
    let updated = store
        .set_wake_on_lan(&entry.id, Some(config.clone()))
        .unwrap();
    assert_eq!(updated.name, "New name");
    assert_eq!(updated.password_key, entry.password_key);
    assert!(updated.password_hint);
    assert_eq!(
        SavedConnections::load(&paths).unwrap().all()[0].wake_on_lan,
        Some(config.clone())
    );
    let mut edited = updated.clone();
    edited.wake_on_lan = None; // ordinary connection form does not carry wake fields
    edited.name = "Edited".into();
    edited.username = "updated-account".into();
    assert_eq!(store.upsert(edited).unwrap().wake_on_lan, Some(config));
    assert!(
        store
            .set_wake_on_lan(&entry.id, None)
            .unwrap()
            .wake_on_lan
            .is_none()
    );
    let old: SavedConnection = serde_json::from_str(r#"{"host":"old.local","port":5900}"#).unwrap();
    assert!(old.wake_on_lan.is_none());
    store.remove(&entry.id).unwrap();
    assert!(
        store
            .set_wake_on_lan(
                &entry.id,
                Some(
                    crate::wake_on_lan::WakeOnLan::parse("021122334455", "255.255.255.255", "9")
                        .unwrap()
                )
            )
            .is_err()
    );
    assert!(store.all().is_empty());
}

#[test]
fn discovery_wake_configuration_is_atomic_and_does_not_clear_existing_credentials() {
    let mut store = SavedConnections::in_memory();
    let target = SavedConnection {
        host: "192.168.1.10".into(),
        port: 5900,
        protocol: ConnectionProtocol::Vnc,
        ..Default::default()
    };
    let invalid = crate::wake_on_lan::WakeOnLan {
        mac_address: "bad".into(),
        destination: std::net::Ipv4Addr::BROADCAST,
        port: 9,
    };
    assert!(
        store
            .configure_discovered_wake(target.clone(), invalid)
            .is_err()
    );
    assert!(store.all().is_empty());
    let mut credentialed = target.clone();
    credentialed.password_hint = true;
    credentialed.password_key = Some("keep-this-secret".into());
    let previous = store.upsert(credentialed).unwrap();
    let config =
        crate::wake_on_lan::WakeOnLan::parse("021122334455", "255.255.255.255", "9").unwrap();
    let configured = store.configure_discovered_wake(target, config).unwrap();
    assert_eq!(configured.id, previous.id);
    assert_eq!(configured.password_key, previous.password_key);
    assert!(configured.password_hint);
    assert_eq!(store.all().len(), 1);
    let mut changed = SavedConnection::from_request(
        &request(ConnectionProtocol::Vnc, "another.local", 5900),
        "Another",
    );
    changed.id = configured.id;
    assert!(store.upsert(changed).unwrap().wake_on_lan.is_none());
}

fn request(protocol: ConnectionProtocol, host: &str, port: u16) -> ConnectionRequest {
    ConnectionRequest {
        video_quality: Default::default(),
        pairing_code: None,
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

#[test]
fn discovery_matches_only_direct_protocol_and_port_using_broadcast_aliases() {
    let mut saved = SavedConnection::from_request(
        &request(ConnectionProtocol::Vnc, "192.168.1.11", 5900),
        "br-gateway",
    );
    let address = ConnectionAddress::parse("192.168.1.11", "5900").unwrap();
    let before = saved.clone();
    assert!(saved.matches_discovered(ConnectionProtocol::Vnc, &address, &[]));
    assert_eq!(
        saved, before,
        "deduplication must not mutate credentials or bookmarks"
    );
    assert!(!saved.matches_discovered(ConnectionProtocol::Rdp, &address, &[]));
    assert!(!saved.matches_discovered(
        ConnectionProtocol::Vnc,
        &ConnectionAddress::parse("192.168.1.11", "5901").unwrap(),
        &[]
    ));
    saved.host = "MAC-MINI.local.".into();
    assert!(!saved.matches_discovered(ConnectionProtocol::Vnc, &address, &[]));
    assert!(saved.matches_discovered(
        ConnectionProtocol::Vnc,
        &address,
        &["mac-mini.local.".into()]
    ));
    saved.host = "[2001:0db8::1]".into();
    assert!(saved.matches_discovered(ConnectionProtocol::Vnc, &address, &["2001:db8::1".into()]));
    saved.host = "fe80::1%8".into();
    assert!(!saved.matches_discovered(ConnectionProtocol::Vnc, &address, &["fe80::1%9".into()]));
    assert!(saved.matches_discovered(
        ConnectionProtocol::Vnc,
        &address,
        &["fe80:0:0:0:0:0:0:1%8".into()]
    ));
    saved.host = address.host.clone();
    saved.relay = Some(
        RelayRoute::parse(
            "removent://relay.example:443",
            crate::connection::RelayTransport::WebSocket,
            "",
            &"aa".repeat(32),
        )
        .unwrap(),
    );
    assert!(!saved.matches_discovered(ConnectionProtocol::Vnc, &address, &[]));
}

#[test]
fn video_quality_is_per_connection_and_old_bookmarks_default_to_standard() {
    use removent_proto::VideoQuality;
    let old: SavedConnection = serde_json::from_str(r#"{"host":"legacy","port":48688}"#).unwrap();
    assert_eq!(old.video_quality, VideoQuality::Standard);
    let mut store = SavedConnections::in_memory();
    for (index, quality) in VideoQuality::ALL.into_iter().enumerate() {
        let mut request = ConnectionRequest::native(([127, 0, 0, 1], 48688 + index as u16).into());
        request.video_quality = quality;
        let entry = store
            .upsert(SavedConnection::from_request(&request, "test"))
            .unwrap();
        let restored: SavedConnection =
            serde_json::from_str(&serde_json::to_string(&entry).unwrap()).unwrap();
        assert_eq!(restored.to_request().video_quality, quality);
    }
    assert_eq!(store.all().len(), 4);
}
