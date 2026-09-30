use removent_core::{DataPaths, Settings};
use removent_daemon::{hostmgr, state::DaemonState};
use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

#[tokio::test]
async fn occupied_port_is_not_reported_ready_and_disable_clears_failure() {
    let dir = tempfile::tempdir().unwrap();
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    let paths = DataPaths {
        root: dir.path().to_owned(),
    };
    paths.ensure_layout().unwrap();
    let settings = Settings {
        host_port: socket.local_addr().unwrap().port(),
        ..Settings::default()
    };
    settings.save(&paths).unwrap();
    let state = Arc::new(DaemonState::new(paths, settings, "test".into()));
    let manager = tokio::spawn(hostmgr::run(state.clone()));
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let status = state.snapshot();
            assert!(status.running);
            assert!(!status.host_ready);
            if status.host_error.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    // A stale asynchronous relay callback cannot claim readiness after disable.
    state.host_ready.store(true, Ordering::SeqCst);
    *state.relay_state.lock().unwrap() = (Some(true), None);
    state.set_enabled(false).unwrap();
    let status = state.snapshot();
    assert!(!status.running && !status.host_ready);
    assert!(status.host_error.is_none());
    assert_eq!(status.relay_connected, Some(false));
    state.shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(2), manager)
        .await
        .unwrap()
        .unwrap();
}
