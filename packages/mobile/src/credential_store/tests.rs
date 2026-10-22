use super::*;

fn request(host: &str) -> Request {
    serde_json::from_value(json!({"protocol":"removent", "host":host, "port":48688})).unwrap()
}

#[test]
fn untrusted_hosts_and_destination_edits_cannot_access_native_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let paths = DataPaths {
        root: dir.path().into(),
    };
    removent_core::identity::load_or_create(&paths, "test").unwrap();
    assert_eq!(
        dispatch(
            &paths,
            Command::Load {
                request: request("mac.local")
            }
        )
        .unwrap(),
        json!([])
    );
    host_pins::remember(&paths, "mac.local:48688", [1; 32], false).unwrap();
    for (host, pin, mode) in [
        ("other.local", [1; 32], AuthenticationMode::Password),
        ("mac.local", [2; 32], AuthenticationMode::Password),
        ("mac.local", [1; 32], AuthenticationMode::Otp),
    ] {
        assert!(
            dispatch(
                &paths,
                Command::Save {
                    request: request(host),
                    original_request: Box::new(request("mac.local")),
                    edits: vec![Credential {
                        host: pin,
                        mode,
                        value: "synthetic".into(),
                        remember: true
                    }],
                }
            )
            .is_err()
        );
    }
}

#[test]
#[ignore = "uses isolated synthetic items in the real Keychain"]
fn native_edit_and_delete_round_trip_keeps_secrets_out_of_bookmarks() {
    let dir = tempfile::tempdir().unwrap();
    let paths = DataPaths {
        root: dir.path().into(),
    };
    let local = removent_core::identity::load_or_create(&paths, "test")
        .unwrap()
        .fingerprint_hex();
    host_pins::remember(&paths, "mac.local:48688", [1; 32], false).unwrap();
    struct Cleanup(Vec<String>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            for key in &self.0 {
                let _ = keychain::delete(key);
            }
        }
    }
    let mut cleanup = Cleanup(Vec::new());
    for mode in [
        AuthenticationMode::SystemAccount,
        AuthenticationMode::Password,
    ] {
        let key = account(&local, [1; 32], mode).unwrap();
        cleanup.0.push(key.clone());
        let value = if mode == AuthenticationMode::SystemAccount {
            removent_proto::SystemCredentials {
                username: "test-owner".into(),
                password: "synthetic".into(),
            }
            .encode()
        } else {
            "synthetic".into()
        };
        keychain::store(&key, &value).unwrap();
    }
    let loaded = dispatch(
        &paths,
        Command::Load {
            request: request("mac.local"),
        },
    )
    .unwrap();
    let mut edits: Vec<Credential> = serde_json::from_value(loaded).unwrap();
    assert_eq!(edits.len(), 2);
    for edit in &mut edits {
        edit.remember = false;
    }
    dispatch(
        &paths,
        Command::Save {
            request: request("mac.local"),
            original_request: Box::new(request("mac.local")),
            edits,
        },
    )
    .unwrap();
    assert_eq!(
        dispatch(
            &paths,
            Command::Load {
                request: request("mac.local")
            }
        )
        .unwrap(),
        json!([])
    );
    assert!(!paths.root.join("connections.json").exists());
}
