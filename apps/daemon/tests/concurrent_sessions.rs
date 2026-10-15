use removent_core::{DataPaths, Settings, ipc::IpcEvent};
use removent_daemon::state::DaemonState;

#[test]
fn ending_one_protocol_preserves_the_other_and_ignores_stale_ends() {
    let directory = tempfile::tempdir().unwrap();
    let state = DaemonState::new(
        DataPaths {
            root: directory.path().into(),
        },
        Settings::default(),
        String::new(),
    );
    let mut events = state.events.subscribe();
    state.session_started(10, "Native viewer".into(), "abcd".into(), "HEVC".into());
    state.session_started(20, "VNC viewer".into(), String::new(), "VNC".into());
    assert_eq!(state.sessions.lock().unwrap().len(), 2);
    assert!(
        matches!(events.try_recv().unwrap(), IpcEvent::SessionStarted { session } if session.id == 10)
    );
    assert!(
        matches!(events.try_recv().unwrap(), IpcEvent::SessionStarted { session } if session.id == 20)
    );
    state.session_ended(10, "closed".into());
    assert_eq!(state.sessions.lock().unwrap()[0].id, 20);
    assert!(matches!(
        events.try_recv().unwrap(),
        IpcEvent::SessionEnded { session_id: 10, .. }
    ));
    state.session_ended(10, "stale".into());
    assert!(events.try_recv().is_err());
    assert_eq!(state.sessions.lock().unwrap().len(), 1);
    state.session_ended(20, "closed".into());
    assert!(state.sessions.lock().unwrap().is_empty());
}
