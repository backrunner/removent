use super::*;
#[cfg(target_os = "macos")]
use std::time::Duration;

#[cfg(target_os = "macos")]
fn test_attempt() -> (ClientAttempt, std::sync::mpsc::Receiver<UiEvent>) {
    let (events, rx) = std::sync::mpsc::channel();
    (
        ClientAttempt {
            generation: 0,
            started: std::time::Instant::now(),
            recovery: Default::default(),
            channels: Default::default(),
            events,
            sessions: Default::default(),
        },
        rx,
    )
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn saved_credentials_connect_without_a_dialog_and_are_not_rewritten() {
    for mode in [
        AuthenticationMode::Password,
        AuthenticationMode::SystemAccount,
    ] {
        let value = if mode == AuthenticationMode::SystemAccount {
            removent_proto::SystemCredentials {
                username: "owner".into(),
                password: "test-password".into(),
            }
            .encode()
        } else {
            "test-password".into()
        };
        let expected = value.clone();
        let (attempt, events) = test_attempt();
        let pending: Pending = Default::default();
        let (tx, rx) = tokio::sync::oneshot::channel();
        prompt_with_loader(
            attempt,
            "local".into(),
            Some([1; 32]),
            pending.clone(),
            move |key| {
                assert_eq!(Some(key.to_owned()), account("local", [1; 32], mode));
                Ok(Some(value))
            },
        )(mode, tx);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), rx)
                .await
                .unwrap()
                .unwrap(),
            expected
        );
        assert!(
            !events
                .try_iter()
                .any(|event| matches!(event, UiEvent::ClientNeedsPin { .. }))
        );
        let error = authentication_failure(
            removent_client::ConnectError::Pairing("denied".into()),
            &pending,
        );
        assert!(
            error
                .to_string()
                .contains(&t!("auth.saved_credentials_failed").to_string())
        );
        let network =
            authentication_failure(removent_client::ConnectError::Timeout("network"), &pending);
        assert!(matches!(
            network,
            removent_client::ConnectError::Timeout("network")
        ));
        pending
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .commit_with(
                "local",
                [1; 32],
                |_, _| panic!("reuse must not overwrite Keychain"),
                |_| panic!("reuse must not delete Keychain"),
            )
            .unwrap();
    }
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn missing_invalid_untrusted_and_temporary_credentials_still_require_input() {
    for (mode, host, stored, should_load) in [
        (AuthenticationMode::Password, Some([1; 32]), None, true),
        (AuthenticationMode::Password, Some([1; 32]), Some(""), true),
        (
            AuthenticationMode::SystemAccount,
            Some([1; 32]),
            Some("not-encoded"),
            true,
        ),
        (
            AuthenticationMode::Password,
            None,
            Some("old-password"),
            false,
        ),
        (
            AuthenticationMode::Otp,
            Some([1; 32]),
            Some("123456"),
            false,
        ),
        (
            AuthenticationMode::PairingCode,
            Some([1; 32]),
            Some("123456"),
            false,
        ),
    ] {
        let (attempt, events) = test_attempt();
        let pending: Pending = Default::default();
        let (tx, rx) = tokio::sync::oneshot::channel();
        prompt_with_loader(attempt, "local".into(), host, pending.clone(), move |_| {
            assert!(should_load);
            Ok(stored.map(str::to_owned))
        })(mode, tx);
        let answer = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(UiEvent::ClientNeedsPin {
                    mode: requested,
                    tx,
                    ..
                }) = events.try_recv()
                {
                    assert_eq!(requested, mode);
                    break tx;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(pending.lock().unwrap().is_none());
        drop(answer);
        assert!(rx.await.is_err());
    }
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn cancelling_a_keychain_read_never_opens_a_dialog_or_submits_a_credential() {
    let (attempt, events) = test_attempt();
    let pending: Pending = Default::default();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let (reading, started) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    prompt_with_loader(
        attempt,
        "local".into(),
        Some([1; 32]),
        pending.clone(),
        move |_| {
            reading.send(()).unwrap();
            blocked.recv().unwrap();
            Ok(Some("test-password".into()))
        },
    )(AuthenticationMode::Password, tx);
    started.await.unwrap();
    drop(rx);
    release.send(()).unwrap();
    // The prompt task drops its event sender on completion.
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match events.try_recv() {
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                Err(std::sync::mpsc::TryRecvError::Empty) => tokio::task::yield_now().await,
                Ok(_) => panic!("cancelled read must not publish UI events"),
            }
        }
    })
    .await
    .unwrap();
    assert!(pending.lock().unwrap().is_none());
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn prompting_keeps_credentials_in_memory_until_success_and_cancellation_discards_them() {
    for cancel in [false, true] {
        let channels = Arc::new(Mutex::new(ClientChannels::default()));
        let (events, rx) = std::sync::mpsc::channel();
        let pending: Pending = Default::default();
        let attempt = ClientAttempt {
            generation: 0,
            started: std::time::Instant::now(),
            recovery: Default::default(),
            channels,
            events,
            sessions: Default::default(),
        };
        let (pin_tx, mut pin_rx) = tokio::sync::oneshot::channel();
        prompt(attempt, "local".into(), None, pending.clone())(
            AuthenticationMode::Password,
            pin_tx,
        );
        let tx = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(UiEvent::ClientNeedsPin { tx, .. }) = rx.try_recv() {
                    break tx;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(pending.lock().unwrap().is_none());
        if cancel {
            drop(tx);
            assert!(pin_rx.await.is_err());
            assert!(pending.lock().unwrap().is_none());
        } else {
            tx.send(AuthenticationAnswer {
                value: "test-password".into(),
                remember: true,
            })
            .unwrap();
            assert_eq!(
                pin_rx.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            );
            assert_eq!(pin_rx.await.unwrap(), "test-password");
            let submitted = pending.lock().unwrap().take().unwrap();
            assert!(
                submitted
                    .commit_with(
                        "local",
                        [1; 32],
                        |_, _| { Err(removent_core::CoreError::SecureStore("test denial".into())) },
                        |_| Ok(())
                    )
                    .is_err()
            );
        }
    }
}

#[cfg(target_os = "macos")]
#[tokio::test]
#[ignore = "uses an isolated synthetic item in the real login Keychain"]
async fn native_credentials_round_trip_in_keychain() {
    let local = format!("qa-{}", std::process::id());
    let host = [71; 32];
    for mode in [
        AuthenticationMode::Password,
        AuthenticationMode::SystemAccount,
    ] {
        let key = account(&local, host, mode).unwrap();
        let value = if mode == AuthenticationMode::SystemAccount {
            removent_proto::SystemCredentials {
                username: "qa-owner".into(),
                password: "synthetic-password".into(),
            }
            .encode()
        } else {
            "synthetic-password".into()
        };
        PendingAuthentication {
            mode,
            reused: false,
            answer: AuthenticationAnswer {
                value: value.clone(),
                remember: true,
            },
        }
        .commit(local.clone(), host)
        .await
        .unwrap();
        let loaded = keychain::load(&key).unwrap();
        PendingAuthentication {
            mode,
            reused: false,
            answer: AuthenticationAnswer {
                value: value.clone(),
                remember: false,
            },
        }
        .commit(local.clone(), host)
        .await
        .unwrap();
        assert_eq!(loaded, Some(value));
        assert_eq!(keychain::load(&key).unwrap(), None);
    }
}

#[test]
fn credentials_are_isolated_by_local_host_and_method_and_debug_is_redacted() {
    let key = account("local", [1; 32], AuthenticationMode::Password).unwrap();
    assert_ne!(
        Some(key.clone()),
        account("other", [1; 32], AuthenticationMode::Password)
    );
    assert_ne!(
        Some(key.clone()),
        account("local", [2; 32], AuthenticationMode::Password)
    );
    assert_ne!(
        Some(key),
        account("local", [1; 32], AuthenticationMode::SystemAccount)
    );
    for mode in [
        AuthenticationMode::PairingCode,
        AuthenticationMode::Otp,
        AuthenticationMode::None,
    ] {
        assert!(account("local", [1; 32], mode).is_none());
        assert!(!can_remember(mode));
    }
    let answer = AuthenticationAnswer {
        value: "secret".into(),
        remember: true,
    };
    assert!(!format!("{answer:?}").contains("secret"));
}

#[test]
fn successful_authentication_stores_or_forgets_only_the_selected_method() {
    for mode in [
        AuthenticationMode::Password,
        AuthenticationMode::SystemAccount,
    ] {
        for remember in [false, true] {
            let value = if mode == AuthenticationMode::SystemAccount {
                removent_proto::SystemCredentials {
                    username: "owner".into(),
                    password: "test-password".into(),
                }
                .encode()
            } else {
                "test-password".into()
            };
            let expected = value.clone();
            PendingAuthentication {
                mode,
                reused: false,
                answer: AuthenticationAnswer { value, remember },
            }
            .commit_with(
                "local",
                [1; 32],
                |key, value| {
                    assert!(remember);
                    assert_eq!(Some(key.to_owned()), account("local", [1; 32], mode));
                    assert_eq!(value, expected);
                    Ok(())
                },
                |key| {
                    assert!(!remember);
                    assert_eq!(Some(key.to_owned()), account("local", [1; 32], mode));
                    Ok(())
                },
            )
            .unwrap();
        }
    }
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "uses isolated synthetic items in the real login Keychain"]
fn edited_credentials_are_used_on_the_next_connection_and_can_be_removed() {
    struct Cleanup(Vec<String>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            for key in &self.0 {
                let _ = keychain::delete(key);
            }
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let paths = DataPaths {
        root: dir.path().into(),
    };
    let (tx, _) = tokio::sync::mpsc::channel(1);
    let engine = Engine::for_ui_test(paths.clone(), tx);
    let request = ConnectionRequest::native("127.0.0.1:48688".parse().unwrap());
    let local = engine.identity().unwrap().fingerprint_hex();
    let host = [73; 32];
    removent_client::host_pins::remember(&paths, &request.address.to_string(), host, false)
        .unwrap();
    let mut cleanup = Cleanup(Vec::new());
    for mode in [
        AuthenticationMode::SystemAccount,
        AuthenticationMode::Password,
    ] {
        let value = if mode == AuthenticationMode::SystemAccount {
            removent_proto::SystemCredentials {
                username: "test-owner".into(),
                password: "old-synthetic-password".into(),
            }
            .encode()
        } else {
            "old-synthetic-password".into()
        };
        let key = account(&local, host, mode).unwrap();
        cleanup.0.push(key.clone());
        keychain::store(&key, &value).unwrap();
    }
    let originals = engine.native_credentials(&request).unwrap();
    assert_eq!(originals.len(), 2);
    let edits: Vec<_> = originals
        .into_iter()
        .map(|original| {
            let value = if original.mode == AuthenticationMode::SystemAccount {
                removent_proto::SystemCredentials {
                    username: "new-owner".into(),
                    password: "new-synthetic-password".into(),
                }
                .encode()
            } else {
                "new-synthetic-password".into()
            };
            NativeCredentialEdit {
                original,
                answer: AuthenticationAnswer {
                    value,
                    remember: true,
                },
            }
        })
        .collect();
    engine.save_native_credentials(&request, &edits).unwrap();
    let saved = engine.native_credentials(&request).unwrap();
    for (credential, edit) in saved.iter().zip(&edits) {
        assert_eq!(credential.answer, edit.answer);
        engine.rt.block_on(async {
            let (attempt, events) = test_attempt();
            let (tx, rx) = tokio::sync::oneshot::channel();
            prompt(attempt, local.clone(), Some(host), Default::default())(credential.mode, tx);
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(2), rx)
                    .await
                    .unwrap()
                    .unwrap(),
                credential.answer.value
            );
            assert!(
                !events
                    .try_iter()
                    .any(|event| matches!(event, UiEvent::ClientNeedsPin { .. }))
            );
        });
    }
    let other = ConnectionRequest::native("127.0.0.2:48688".parse().unwrap());
    assert!(engine.save_native_credentials(&other, &edits).is_err());
    removent_client::host_pins::remember(&paths, &request.address.to_string(), [74; 32], true)
        .unwrap();
    assert!(engine.save_native_credentials(&request, &edits).is_err());
    removent_client::host_pins::remember(&paths, &request.address.to_string(), host, true).unwrap();
    let mut edits = edits;
    for edit in &mut edits {
        edit.answer.remember = false;
    }
    engine.save_native_credentials(&request, &edits).unwrap();
    assert!(engine.native_credentials(&request).unwrap().is_empty());
    for key in &cleanup.0 {
        assert_eq!(keychain::load(key).unwrap(), None);
    }
}
