use super::*;

#[tokio::test]
async fn local_and_viewer_held_state_cannot_release_each_other() {
    let control = Control::default();
    let token = CancellationToken::new();
    let active = control.begin("active", 0, token.clone()).unwrap();
    control.local_input(true);
    assert!(token.is_cancelled());
    assert!(active.interrupted());
    drop(active);
    drop(control.manual_input(false).await.unwrap());
    assert_eq!(control.status()["input_held"], true);
    assert_eq!(control.status()["local_input_held"], true);
    assert_eq!(control.status()["viewer_input_held"], false);
    drop(control.manual_input(true).await.unwrap());
    control.local_input(false);
    assert_eq!(control.status()["input_held"], true);
    assert_eq!(control.status()["viewer_input_held"], true);
    assert_eq!(control.status()["local_input_held"], false);
    assert_eq!(control.status()["last_input_source"], "local");
    assert_eq!(
        control.resume(control.revision()).unwrap_err().code,
        "manual_busy"
    );
    drop(control.manual_input(false).await.unwrap());
    tokio::time::sleep(MANUAL_IDLE + Duration::from_millis(10)).await;
    let revision = control.revision();
    assert_eq!(control.resume(revision).unwrap()["mode"], "automation");
    assert_eq!(
        control
            .begin("late", revision, CancellationToken::new())
            .err()
            .unwrap()
            .code,
        "control_changed"
    );
}

#[test]
fn monitoring_must_be_ready_and_a_gap_requires_a_new_session() {
    let control = Control::default();
    control.require_monitor();
    assert!(!control.can_mutate());
    assert_eq!(
        control
            .begin("too-early", 0, CancellationToken::new())
            .err()
            .unwrap()
            .code,
        "local_monitor_unavailable"
    );
    control.monitor_started(false);
    assert!(control.can_mutate());
    let token = CancellationToken::new();
    let active = control.begin("active", 0, token.clone()).unwrap();
    control.monitor_failed("secure_input");
    assert!(token.is_cancelled());
    drop(active);
    assert!(!control.can_mutate());
    assert_eq!(control.status()["last_input_source"], "monitor");
    assert_eq!(
        control.resume(control.revision()).unwrap_err().code,
        "local_monitor_unavailable"
    );
    // Recovering permission or receiving a late startup callback never silently
    // erases the interval in which local releases could have been missed.
    control.monitor_started(false);
    control.monitor_heartbeat();
    control.monitor_failed("stopped");
    assert_eq!(control.monitor_status()["reason"], "secure_input");
    assert!(!control.monitor_ready());
}

#[test]
fn expired_heartbeat_cancels_active_work_and_late_heartbeats_cannot_recover() {
    let control = Control::default();
    control.require_monitor();
    control.monitor_started(false);
    let token = CancellationToken::new();
    let _active = control.begin("active", 0, token.clone()).unwrap();
    control.state.lock().unwrap().monitor = Monitor::Ready(Instant::now() - Duration::from_secs(1));
    assert!(!control.can_mutate());
    assert!(!control.monitor_ready());
    assert!(control.expire_monitor());
    assert!(token.is_cancelled());
    control.monitor_heartbeat();
    assert_eq!(control.monitor_status()["reason"], "monitor_unresponsive");
}

#[test]
fn late_listener_heartbeat_cannot_hide_a_gap_even_before_watchdog_runs() {
    let control = Control::default();
    control.require_monitor();
    control.monitor_started(false);
    let token = CancellationToken::new();
    let _active = control.begin("active", 0, token.clone()).unwrap();
    control.state.lock().unwrap().monitor = Monitor::Ready(Instant::now() - Duration::from_secs(1));
    assert!(!control.monitor_heartbeat());
    assert!(token.is_cancelled());
    assert_eq!(control.monitor_status()["reason"], "monitor_unresponsive");
}

#[test]
fn startup_is_bounded_and_initially_held_physical_input_pauses_automation() {
    let timed_out = Control::default();
    timed_out.require_monitor();
    timed_out.state.lock().unwrap().monitor =
        Monitor::Starting(Instant::now() - Duration::from_secs(3));
    assert!(timed_out.expire_monitor());
    assert_eq!(timed_out.monitor_status()["reason"], "startup_timeout");
    let held = Control::default();
    held.require_monitor();
    held.monitor_started(true);
    assert!(held.monitor_ready());
    assert!(!held.can_mutate());
    assert_eq!(held.status()["mode"], "manual");
    assert_eq!(held.status()["local_input_held"], true);
    assert_eq!(held.status()["last_input_source"], "local");
}
