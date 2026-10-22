use super::*;

fn fixture() -> (tempfile::TempDir, Engine, Attempt) {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::new(dir.path().to_str().unwrap(), "CredentialTest").unwrap();
    let attempt = Attempt {
        shared: engine.shared.clone(),
        generation: 0,
        recovery: Default::default(),
    };
    (dir, engine, attempt)
}

#[test]
fn native_keys_match_desktop_and_isolate_identities_and_modes() {
    let key = account("local", [1; 32], AuthenticationMode::Password).unwrap();
    assert_eq!(
        key,
        format!("native-auth:v1:local:{}:password", hex::encode([1; 32]))
    );
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
        AuthenticationMode::Otp,
        AuthenticationMode::PairingCode,
        AuthenticationMode::None,
    ] {
        assert!(account("local", [1; 32], mode).is_none());
    }
}

#[tokio::test]
async fn stored_credentials_authenticate_without_prompt_or_rewrite() {
    for mode in [
        AuthenticationMode::Password,
        AuthenticationMode::SystemAccount,
    ] {
        let (_dir, engine, attempt) = fixture();
        let pending: Pending = Default::default();
        let (tx, rx) = oneshot::channel();
        let value = if mode == AuthenticationMode::SystemAccount {
            removent_proto::SystemCredentials {
                username: "owner".into(),
                password: "synthetic".into(),
            }
            .encode()
        } else {
            "synthetic".into()
        };
        let expected = value.clone();
        prompt_with_loader(attempt, Some([1; 32]), pending.clone(), move |_| {
            Ok(Some(value))
        })(mode, tx);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), rx)
                .await
                .unwrap()
                .unwrap(),
            expected
        );
        assert!(
            !engine
                .shared
                .state
                .lock()
                .unwrap()
                .events
                .iter()
                .any(|event| event["type"] == "pin")
        );
        let error = authentication_failure(
            removent_client::ConnectError::Pairing("denied".into()),
            &pending,
        );
        assert!(
            error
                .to_string()
                .contains("saved_native_credentials_failed")
        );
        let auth = pending.lock().unwrap().take().unwrap();
        assert!(auth.reused);
        auth.commit("not-a-keychain-item".into(), [0; 32])
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn temporary_untrusted_missing_and_invalid_values_prompt_without_saving() {
    for (mode, host, value, load_expected) in [
        (AuthenticationMode::Password, None, Some("old"), false),
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
        (AuthenticationMode::Password, Some([1; 32]), None, true),
        (
            AuthenticationMode::SystemAccount,
            Some([1; 32]),
            Some("invalid"),
            true,
        ),
    ] {
        let (_dir, engine, attempt) = fixture();
        let pending: Pending = Default::default();
        let (tx, rx) = oneshot::channel();
        prompt_with_loader(attempt, host, pending.clone(), move |_| {
            assert!(load_expected);
            Ok(value.map(str::to_owned))
        })(mode, tx);
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if engine.shared.state.lock().unwrap().pin.is_some() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(pending.lock().unwrap().is_none());
        drop(rx);
    }
}

#[tokio::test]
async fn cancelled_keychain_read_cannot_submit_or_open_a_prompt() {
    let (_dir, mut engine, attempt) = fixture();
    let pending: Pending = Default::default();
    let (tx, rx) = oneshot::channel();
    let (reading, started) = oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    prompt_with_loader(attempt, Some([1; 32]), pending.clone(), move |_| {
        reading.send(()).unwrap();
        blocked.recv().unwrap();
        Ok(Some("synthetic".into()))
    })(AuthenticationMode::Password, tx);
    started.await.unwrap();
    drop(rx);
    engine.stop();
    release.send(()).unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(pending.lock().unwrap().is_none());
    assert!(engine.shared.state.lock().unwrap().events.is_empty());
}
