use super::*;
#[test]
fn cancelled_attempt_cannot_publish_frames_pins_or_ready() {
    let dir = tempfile::tempdir().unwrap();
    let mut engine = Engine::new(dir.path().to_str().unwrap(), "test").unwrap();
    let old = Attempt {
        shared: engine.shared.clone(),
        generation: 0,
    };
    engine.stop();
    old.frame(DecodedFrame {
        data: vec![0; 4],
        width: 1,
        height: 1,
        pts_us: 0,
    });
    old.event(json!({"type":"ready"}));
    old.finish(Ok(()));
    let s = engine.shared.state.lock().unwrap();
    assert!(s.frame.is_none());
    assert!(s.events.is_empty());
}
#[test]
fn invalid_new_request_does_not_cancel_existing_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let mut engine = Engine::new(dir.path().to_str().unwrap(), "test").unwrap();
    let command = serde_json::from_value(json!({"op":"connect", "request":{
        "protocol":"vnc", "host":"", "port":0 }}))
    .unwrap();
    assert!(engine.command(command).is_err());
    assert_eq!(engine.shared.state.lock().unwrap().generation, 0);
}

#[test]
fn saved_credentials_are_scoped_to_the_computer_and_account() {
    let request: Request = serde_json::from_value(json!({
        "protocol":"rdp", "host":"pc.local", "port":3389, "username":"alice"
    }))
    .unwrap();
    let connection = request.connection().unwrap();
    let entry = SavedConnection::from_request(&connection, "Before");
    assert!(same_credential_scope(&entry, &connection));
    let mut changed = request.connection().unwrap();
    changed.address.host = "other.local".into();
    assert!(!same_credential_scope(&entry, &changed));
    changed = request.connection().unwrap();
    changed.address.port += 1;
    assert!(!same_credential_scope(&entry, &changed));
    changed = request.connection().unwrap();
    changed.username = "bob".into();
    assert!(!same_credential_scope(&entry, &changed));
    changed = request.connection().unwrap();
    changed.domain = "other".into();
    assert!(!same_credential_scope(&entry, &changed));
    changed = request.connection().unwrap();
    changed.protocol = ConnectionProtocol::Vnc;
    assert!(!same_credential_scope(&entry, &changed));
}

#[test]
fn form_validation_never_starts_or_cancels_a_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut engine = Engine::new(dir.path().to_str().unwrap(), "test").unwrap();
    let command = serde_json::from_value(json!({"op":"validate", "request":{
        "protocol":"vnc", "host":"pc.local", "port":5900 }}))
    .unwrap();
    assert!(engine.command(command).is_ok());
    assert_eq!(engine.shared.state.lock().unwrap().generation, 0);
    assert!(engine.task.is_none());
}

#[test]
fn authentication_input_is_validated_by_host_mode_and_stale_generations_cannot_consume_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut engine = Engine::new(dir.path().to_str().unwrap(), "AuthMobile").unwrap();
    let (tx, mut rx) = oneshot::channel();
    {
        let mut state = engine.shared.state.lock().unwrap();
        state.auth_mode = removent_core::AuthenticationMode::Password;
        state.pin = Some(tx);
    }
    assert!(
        engine
            .command(Command::Pin {
                generation: 99,
                pin: "correct-password".into()
            })
            .is_err()
    );
    assert!(
        engine
            .command(Command::Pin {
                generation: 0,
                pin: "".into()
            })
            .is_err()
    );
    engine
        .command(Command::Pin {
            generation: 0,
            pin: "correct-password".into(),
        })
        .unwrap();
    assert_eq!(rx.try_recv().unwrap(), "correct-password");
    let (tx, mut rx) = oneshot::channel();
    {
        let mut state = engine.shared.state.lock().unwrap();
        state.auth_mode = removent_core::AuthenticationMode::Otp;
        state.pin = Some(tx);
    }
    assert!(
        engine
            .command(Command::Pin {
                generation: 0,
                pin: "password".into()
            })
            .is_err()
    );
    engine
        .command(Command::Pin {
            generation: 0,
            pin: "123456".into(),
        })
        .unwrap();
    assert_eq!(rx.try_recv().unwrap(), "123456");
}

#[test]
fn certificate_confirmation_cannot_be_consumed_by_a_stale_attempt_or_survive_cancellation() {
    let dir = tempfile::tempdir().unwrap();
    let mut engine = Engine::new(dir.path().to_str().unwrap(), "TrustMobile").unwrap();
    let (tx, mut rx) = oneshot::channel();
    engine.shared.state.lock().unwrap().certificate = Some(tx);
    assert!(
        engine
            .command(Command::ConfirmCertificate {
                generation: 99,
                accept: true
            })
            .is_err()
    );
    assert!(matches!(
        rx.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    engine
        .command(Command::ConfirmCertificate {
            generation: 0,
            accept: false,
        })
        .unwrap();
    assert!(!rx.try_recv().unwrap());
    let (tx, mut rx) = oneshot::channel();
    engine.shared.state.lock().unwrap().certificate = Some(tx);
    engine.stop();
    assert!(matches!(
        rx.try_recv(),
        Err(oneshot::error::TryRecvError::Closed)
    ));
    let old = Attempt {
        shared: engine.shared.clone(),
        generation: 0,
    };
    let (tx, mut rx) = oneshot::channel();
    (old.certificate_confirmation("office".into(), false))([1; 32], tx);
    assert!(matches!(
        rx.try_recv(),
        Err(oneshot::error::TryRecvError::Closed)
    ));
    assert!(engine.shared.state.lock().unwrap().events.is_empty());
}
