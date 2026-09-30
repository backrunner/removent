use super::*;

#[test]
fn update_channel_switch_is_persistent_and_transactional() {
    use removent_core::UpdateChannel::{Beta, Stable};
    let dir = tempfile::tempdir().unwrap();
    let paths = DataPaths {
        root: dir.path().into(),
    };
    let engine = Engine::with_state(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap(),
        paths.clone(),
        Settings::default(),
    );
    engine.set_update_channel(Beta).unwrap();
    assert_eq!(engine.settings().update_channel, Beta);
    assert_eq!(Settings::load(&paths).unwrap().update_channel, Beta);
    engine.update.lock().unwrap().status = UpdateStatus::Downloading {
        version: "9.0.0-beta.1".into(),
    };
    assert!(engine.set_update_channel(Stable).is_err());
    assert_eq!(Settings::load(&paths).unwrap().update_channel, Beta);
    engine.update.lock().unwrap().status = UpdateStatus::ReadyToInstall {
        version: "9.0.0-beta.1".into(),
    };
    engine.set_update_channel(Stable).unwrap();
    assert_eq!(engine.update_status(), UpdateStatus::Idle);
    assert_eq!(Settings::load(&paths).unwrap().update_channel, Stable);
    std::fs::remove_file(paths.settings_file()).unwrap();
    std::fs::create_dir(paths.settings_file()).unwrap();
    assert!(engine.set_update_channel(Beta).is_err());
    assert_eq!(engine.settings().update_channel, Stable);
    assert_eq!(engine.update_status(), UpdateStatus::Idle);
}

#[test]
fn discovery_updates_publish_only_after_successful_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let paths = DataPaths {
        root: dir.path().into(),
    };
    let engine = Engine::with_state(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap(),
        paths.clone(),
        Settings::default(),
    );
    let mut changes = engine.discovery_settings.subscribe();
    engine.update_settings(|s| s.discovery.vnc = true).unwrap();
    assert!(changes.has_changed().unwrap());
    assert!(changes.borrow_and_update().vnc);
    assert!(Settings::load(&paths).unwrap().discovery.vnc);
    engine
        .update_settings(|s| s.theme = removent_core::Theme::Dark)
        .unwrap();
    assert!(!changes.has_changed().unwrap());
    std::fs::remove_file(paths.settings_file()).unwrap();
    std::fs::create_dir(paths.settings_file()).unwrap();
    assert!(engine.update_settings(|s| s.discovery.vnc = false).is_err());
    assert!(engine.settings().discovery.vnc);
    assert!(!changes.has_changed().unwrap());
}

fn attempt(
    channels: &Arc<Mutex<ClientChannels>>,
    events: &std::sync::mpsc::Sender<UiEvent>,
) -> ClientAttempt {
    ClientAttempt {
        generation: channels.lock().unwrap().invalidate(),
        started: std::time::Instant::now(),
        channels: channels.clone(),
        events: events.clone(),
    }
}

#[test]
fn reconnect_pauses_input_without_invalidating_viewer_or_new_attempt() {
    let channels = Arc::new(Mutex::new(ClientChannels::default()));
    let (events, _rx) = std::sync::mpsc::channel();
    let old = attempt(&channels, &events);
    let (cmd, _cmd_rx) = tokio::sync::mpsc::channel(4);
    let _frames = old.publish(cmd, "test".into()).unwrap();
    old.pause_input();
    assert!(channels.lock().unwrap().cmd.is_none());
    assert!(channels.lock().unwrap().frames.is_some());
    let (cmd, _cmd_rx) = tokio::sync::mpsc::channel(4);
    channels.lock().unwrap().geometry = Some((160, 120));
    old.resume(cmd).unwrap();
    assert!(
        channels.lock().unwrap().geometry.is_none(),
        "resume must resend viewer geometry"
    );
    assert!(channels.lock().unwrap().cmd.is_some());
    let new = attempt(&channels, &events);
    let (cmd, _cmd_rx) = tokio::sync::mpsc::channel(4);
    let _frames = new.publish(cmd, "test".into()).unwrap();
    old.pause_input();
    assert!(channels.lock().unwrap().cmd.is_some());
}

#[test]
fn motion_flood_reserves_capacity_for_ordered_key_releases() {
    use removent_proto::{KeyKind, KeyModifiers, MouseKind};
    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    let key = |kind| ControlMsg::KeyEvent {
        vk_code: 0,
        modifiers: KeyModifiers::empty(),
        kind,
        unicode: None,
    };
    enqueue_input(&tx, key(KeyKind::Down)).unwrap();
    for _ in 0..1000 {
        enqueue_input(
            &tx,
            ControlMsg::MouseEvent {
                display_id: 0,
                x_px: 1.,
                y_px: 1.,
                buttons: 0,
                kind: MouseKind::Moved,
            },
        )
        .unwrap();
    }
    enqueue_input(&tx, key(KeyKind::Up)).unwrap();
    assert!(matches!(
        rx.try_recv().unwrap(),
        ControlMsg::KeyEvent {
            kind: KeyKind::Down,
            ..
        }
    ));
    while let Ok(msg) = rx.try_recv() {
        if matches!(
            msg,
            ControlMsg::KeyEvent {
                kind: KeyKind::Up,
                ..
            }
        ) {
            assert!(rx.try_recv().is_err());
            return;
        }
    }
    panic!("key release was lost");
}

#[test]
fn transition_overflow_and_closed_channel_are_reported() {
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let release = || ControlMsg::KeyEvent {
        vk_code: 0,
        modifiers: removent_proto::KeyModifiers::empty(),
        kind: removent_proto::KeyKind::Up,
        unicode: None,
    };
    enqueue_input(&tx, release()).unwrap();
    assert!(matches!(
        enqueue_input(&tx, release()),
        Err(tokio::sync::mpsc::error::TrySendError::Full(_))
    ));
    drop(rx);
    assert!(matches!(
        enqueue_input(&tx, release()),
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_))
    ));
}

#[test]
fn cancellation_invalidates_queued_progress_ready_pin_and_failure_events() {
    let channels = Arc::new(Mutex::new(ClientChannels::default()));
    let (events, rx) = std::sync::mpsc::channel();
    let old = attempt(&channels, &events);
    let (cmd, _) = tokio::sync::mpsc::channel(1);
    let frames = old.publish(cmd, "RDP".into()).unwrap();
    let (tx, _) = tokio::sync::oneshot::channel();
    events
        .send(UiEvent::ClientNeedsPin {
            generation: old.generation,
            tx,
        })
        .unwrap();
    old.progress(ConnectionStage::Authenticating);
    old.finish(Some("old failure".into()));

    let cancelled = channels.lock().unwrap().invalidate();
    assert!(frames.is_closed());
    let queued: Vec<_> = rx.try_iter().collect();
    assert_eq!(queued.len(), 4);
    assert!(
        queued
            .iter()
            .all(|event| !event.belongs_to_client(cancelled))
    );
    assert!(UiEvent::Notice("daemon notice".into()).belongs_to_client(cancelled));
}

#[test]
fn cancelled_task_cannot_publish_or_clear_replacement_channels() {
    let channels = Arc::new(Mutex::new(ClientChannels::default()));
    let (events, rx) = std::sync::mpsc::channel();
    let old = attempt(&channels, &events);
    let current = attempt(&channels, &events);
    let (cmd, _) = tokio::sync::mpsc::channel(1);
    let current_frames = current.publish(cmd.clone(), "RDP".into()).unwrap();
    assert!(old.publish(cmd.clone(), "VNC".into()).is_err());
    assert!(old.resume(cmd.clone()).is_err());
    old.progress(ConnectionStage::PreparingDesktop);
    old.finish(Some("cancelled task failed".into()));
    assert!(!current_frames.is_closed());
    assert!(
        matches!(channels.lock().unwrap().cmd.as_ref(), Some(ClientCommands::Standard(tx)) if tx.same_channel(&cmd))
    );
    assert_eq!(rx.try_iter().count(), 1);
    current.finish(None);
    assert!(channels.lock().unwrap().cmd.is_none());
    assert!(rx.try_recv().unwrap().belongs_to_client(current.generation));
}
