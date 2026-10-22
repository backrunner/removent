use super::*;

#[test]
fn daemon_restart_republishes_unchanged_permissions_and_then_live_changes() {
    let directory = tempfile::tempdir_in("/tmp").unwrap();
    let paths = DataPaths {
        root: directory.path().to_owned(),
    };
    paths.ensure_layout().unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let engine = Engine::with_state(runtime, paths.clone(), Settings::default());
    let receiver = engine.events_rx.lock().unwrap().take().unwrap();
    let server = engine.rt.spawn(async move {
        let listener = tokio::net::UnixListener::bind(paths.daemon_socket()).unwrap();
        for _ in 0..2 {
            let (socket, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = socket.into_split();
            let mut reader = tokio::io::BufReader::new(reader);
            let _: IpcRequest = removent_core::ipc::read_msg(&mut reader)
                .await
                .unwrap()
                .unwrap();
            let report = StatusReport {
                screen_recording_granted: true,
                accessibility_granted: false,
                ..Default::default()
            };
            // Repeated polls within this connection must not spam the UI.
            let response = IpcResponse::Status(Box::new(report));
            for _ in 0..2 {
                removent_core::ipc::write_msg(&mut writer, &response)
                    .await
                    .unwrap();
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        // A final daemon reports a grant changed in System Settings.
        let (socket, _) = listener.accept().await.unwrap();
        let (reader, mut writer) = socket.into_split();
        let mut reader = tokio::io::BufReader::new(reader);
        let _: IpcRequest = removent_core::ipc::read_msg(&mut reader)
            .await
            .unwrap()
            .unwrap();
        let report = StatusReport {
            screen_recording_granted: true,
            accessibility_granted: true,
            ..Default::default()
        };
        removent_core::ipc::write_msg(&mut writer, &IpcResponse::Status(Box::new(report)))
            .await
            .unwrap();
    });
    engine.spawn_daemon_link();
    // Three daemon connections include two retry delays; leave room for CI
    // runners doing release builds alongside this test.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let mut grants = Vec::new();
    while grants.len() < 3 {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        match receiver
            .recv_timeout(remaining)
            .expect("daemon link must refresh grants after each restart")
        {
            UiEvent::DaemonPermissions {
                screen_recording,
                accessibility,
            } => {
                grants.push((screen_recording, accessibility));
            }
            UiEvent::DaemonOnline(false) => {
                assert!(engine.daemon_perms.lock().unwrap().is_none());
            }
            _ => {}
        }
    }
    assert_eq!(grants, [(true, false), (true, false), (true, true)]);
    engine.rt.block_on(server).unwrap();
}

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
    let generation = channels.lock().unwrap().invalidate();
    let recovery = channels.lock().unwrap().recovery.clone();
    ClientAttempt {
        generation,
        started: std::time::Instant::now(),
        recovery,
        channels: channels.clone(),
        events: events.clone(),
        sessions: Default::default(),
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
    old.publish(cmd, "test".into()).unwrap();
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
fn reconnect_keeps_the_original_viewer_frame_channel_and_reports_final_error() {
    let channels = Arc::new(Mutex::new(ClientChannels::default()));
    let (events, _rx) = std::sync::mpsc::channel();
    let attempt = attempt(&channels, &events);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (cmd, _cmd_rx) = tokio::sync::mpsc::channel(4);
        let original = attempt.publish(cmd, "VNC".into()).unwrap();
        let mut frames = channels.lock().unwrap().frames.take().unwrap();
        drop(original);
        attempt.pause_input();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), frames.recv())
                .await
                .is_err()
        );
        let (cmd, _cmd_rx) = tokio::sync::mpsc::channel(4);
        let replacement = attempt.publish(cmd, "VNC".into()).unwrap();
        assert!(
            channels.lock().unwrap().frames.is_none(),
            "must not open a second viewer"
        );
        replacement
            .send(VideoFrame {
                width: 1,
                height: 1,
                data: vec![3; 4],
                pts_us: 0,
            })
            .unwrap();
        assert_eq!(frames.recv().await.unwrap().data, vec![3; 4]);
        drop(replacement);
        attempt.finish(Some("retry budget exhausted".into()));
        assert!(matches!(
            attempt.recovery.state(),
            removent_client::recovery::State::Failed(_)
        ));
        assert!(frames.recv().await.is_none());
    });
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
    old.progress(ConnectionStage::Authenticating);
    let frames = old.publish(cmd, "RDP".into()).unwrap();
    let (tx, _) = tokio::sync::oneshot::channel();
    events
        .send(UiEvent::ClientNeedsPin {
            generation: old.generation,
            mode: Default::default(),
            tx,
        })
        .unwrap();
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
