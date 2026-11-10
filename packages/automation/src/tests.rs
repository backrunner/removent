use super::*;
struct Counter(usize);
impl Provider for Counter {
    fn execute(&mut self, _: &Command, _: &CancellationToken) -> Outcome {
        self.0 += 1;
        Ok(serde_json::json!({"executed":self.0}))
    }
}
#[test]
fn duplicate_mutations_are_not_replayed_and_conflicts_fail() {
    let mut ledger = Ledger::default();
    let mut provider = Counter(0);
    let mut req = Request {
        control_revision: None,
        version: 1,
        operation_id: "same".into(),
        command: Command::TypeText { text: "a".into() },
    };
    let cancel = CancellationToken::new();
    assert!(
        ledger
            .run(&req, &cancel, || provider.execute(&req.command, &cancel))
            .is_ok()
    );
    assert!(
        ledger
            .run(&req, &cancel, || provider.execute(&req.command, &cancel))
            .is_ok()
    );
    assert_eq!(provider.0, 1);
    req.command = Command::TypeText { text: "b".into() };
    assert_eq!(
        ledger
            .run(&req, &cancel, || provider.execute(&req.command, &cancel))
            .unwrap_err()
            .code,
        "operation_conflict"
    );
    req.operation_id = "cancelled".into();
    cancel.cancel();
    assert!(
        ledger
            .run(&req, &cancel, || provider.execute(&req.command, &cancel))
            .is_err()
    );
    assert_eq!(provider.0, 1);
}

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;
struct HeldProvider {
    held: Arc<AtomicBool>,
    effects: Arc<AtomicUsize>,
    invalidations: Arc<AtomicUsize>,
}
impl Provider for HeldProvider {
    fn execute(&mut self, command: &Command, cancel: &CancellationToken) -> Outcome {
        if matches!(command, Command::PressKey { .. }) {
            self.held.store(true, Ordering::SeqCst);
            while !cancel.is_cancelled() {
                std::thread::sleep(Duration::from_millis(2));
            }
            // Model release latency: manual input must wait for this cleanup.
            std::thread::sleep(Duration::from_millis(30));
            self.held.store(false, Ordering::SeqCst);
        }
        if command.mutates() {
            self.effects.fetch_add(1, Ordering::SeqCst);
        }
        Ok(serde_json::json!({"done":true}))
    }
    fn invalidate_observations(&mut self) {
        self.invalidations.fetch_add(1, Ordering::SeqCst);
    }
}
fn request(id: &str, command: Command) -> Request {
    Request {
        version: 1,
        operation_id: id.into(),
        control_revision: None,
        command,
    }
}
async fn await_held(held: &AtomicBool) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while !held.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test(flavor = "multi_thread")]
async fn takeover_settles_input_rejects_queued_work_and_requires_explicit_fresh_revision() {
    let cancel = CancellationToken::new();
    let _stop = cancel.clone().drop_guard();
    let held = Arc::new(AtomicBool::new(false));
    let effects = Arc::new(AtomicUsize::new(0));
    let invalidations = Arc::new(AtomicUsize::new(0));
    let provider = HeldProvider {
        held: held.clone(),
        effects: effects.clone(),
        invalidations: invalidations.clone(),
    };
    let executor = Executor::spawn(cancel.clone(), move || (provider, ())).unwrap();
    let first = executor.clone();
    let token = cancel.child_token();
    let active = tokio::spawn(async move {
        first
            .execute(
                request(
                    "held",
                    Command::PressKey {
                        key: "a".into(),
                        hold_ms: 3000,
                    },
                ),
                token,
            )
            .await
    });
    await_held(&held).await;
    let queued = executor.clone();
    let token = cancel.child_token();
    let waiting = tokio::spawn(async move {
        queued
            .execute(
                request(
                    "queued",
                    Command::TypeText {
                        text: "never".into(),
                    },
                ),
                token,
            )
            .await
    });
    // Queue the second operation before the takeover, without relying on a scheduling delay.
    while executor.tx.capacity() == 4 {
        tokio::task::yield_now().await;
    }
    drop(executor.manual_input(true).await.unwrap());
    assert!(!held.load(Ordering::SeqCst));
    assert_eq!(active.await.unwrap().unwrap_err().code, "control_changed");
    assert_eq!(waiting.await.unwrap().unwrap_err().code, "control_changed");
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    let status = executor
        .execute(
            request("status", Command::ControlStatus {}),
            cancel.child_token(),
        )
        .await
        .unwrap();
    assert_eq!(status["mode"], "manual");
    assert_eq!(status["input_held"], true);
    let rev = status["revision"].as_u64().unwrap();
    assert_eq!(
        executor
            .execute(
                request("busy", Command::ResumeControl { revision: rev }),
                cancel.child_token()
            )
            .await
            .unwrap_err()
            .code,
        "manual_busy"
    );
    drop(executor.manual_input(false).await.unwrap());
    tokio::time::sleep(Duration::from_millis(510)).await;
    assert_eq!(
        executor
            .execute(
                request("stale-resume", Command::ResumeControl { revision: rev }),
                cancel.child_token()
            )
            .await
            .unwrap_err()
            .code,
        "control_changed"
    );
    let current = executor.control.revision();
    let resumed = executor
        .execute(
            request("resume", Command::ResumeControl { revision: current }),
            cancel.child_token(),
        )
        .await
        .unwrap();
    let revision = resumed["revision"].as_u64().unwrap();
    assert_eq!(resumed["mode"], "automation");
    // Delayed pre-takeover traffic has no current control revision and cannot inject.
    assert_eq!(
        executor
            .execute(
                request(
                    "late",
                    Command::TypeText {
                        text: "late".into()
                    }
                ),
                cancel.child_token()
            )
            .await
            .unwrap_err()
            .code,
        "control_changed"
    );
    let mut fresh = request("fresh", Command::TypeText { text: "new".into() });
    fresh.control_revision = Some(revision);
    executor.execute(fresh, cancel.child_token()).await.unwrap();
    assert_eq!(effects.load(Ordering::SeqCst), 2);
    // Queued work remains rejected even when somebody retries it after resume.
    assert_eq!(
        executor
            .execute(
                request(
                    "queued",
                    Command::TypeText {
                        text: "never".into()
                    }
                ),
                cancel.child_token()
            )
            .await
            .unwrap_err()
            .code,
        "control_changed"
    );
    assert!(invalidations.load(Ordering::SeqCst) >= 2);
}
#[tokio::test]
async fn uncooperative_action_keeps_manual_input_blocked_until_cleanup() {
    let control = control::Control::default();
    let token = CancellationToken::new();
    let active = control.begin("stuck", 0, token.clone()).unwrap();
    assert_eq!(
        control.manual_input(false).await.unwrap_err().code,
        "handover_timeout"
    );
    assert!(token.is_cancelled());
    assert_eq!(control.status()["mode"], "manual");
    assert_eq!(control.resume(1).unwrap_err().code, "manual_busy");
    drop(active);
    tokio::time::sleep(Duration::from_millis(510)).await;
    assert_eq!(control.resume(1).unwrap()["mode"], "automation");
}

#[tokio::test]
async fn resume_waits_for_viewer_dispatch_even_after_slow_native_cleanup() {
    let control = control::Control::default();
    let manual = control.manual_input(false).await.unwrap();
    // Even after the idle interval, a pending viewer event keeps the gate closed.
    tokio::time::sleep(Duration::from_millis(510)).await;
    assert_eq!(control.status()["input_pending"], true);
    assert_eq!(control.resume(1).unwrap_err().code, "manual_busy");
    drop(manual);
    assert_eq!(control.status()["input_pending"], false);
    assert_eq!(control.resume(1).unwrap_err().code, "manual_busy");
    tokio::time::sleep(Duration::from_millis(510)).await;
    assert_eq!(control.resume(1).unwrap()["mode"], "automation");
}

#[tokio::test(flavor = "multi_thread")]
async fn unavailable_local_monitor_keeps_reads_but_never_calls_mutating_provider() {
    let cancel = CancellationToken::new();
    let _stop = cancel.clone().drop_guard();
    let control = control::Control::default();
    control.require_monitor();
    control.monitor_failed("input_monitoring_permission");
    let revision = control.revision();
    let executor = Executor::spawn_control(cancel.clone(), control, || (Counter(0), ())).unwrap();
    let read = executor
        .execute(request("read", Command::State {}), cancel.child_token())
        .await
        .unwrap();
    assert_eq!(read["executed"], 1);
    let mut denied = request(
        "denied",
        Command::TypeText {
            text: "must not run".into(),
        },
    );
    denied.control_revision = Some(revision);
    assert_eq!(
        executor
            .execute(denied, cancel.child_token())
            .await
            .unwrap_err()
            .code,
        "local_monitor_unavailable"
    );
    assert_eq!(
        executor
            .execute(
                request("resume", Command::ResumeControl { revision }),
                cancel.child_token()
            )
            .await
            .unwrap_err()
            .code,
        "local_monitor_unavailable"
    );
    let read = executor
        .execute(
            request("still-readable", Command::State {}),
            cancel.child_token(),
        )
        .await
        .unwrap();
    assert_eq!(read["executed"], 2);
}
