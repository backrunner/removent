//! Host-authoritative arbitration between viewer input and automation.
use crate::Outcome;
use removent_proto::automation::Failure;
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
mod local;
pub(crate) use local::LocalHeld;

const MANUAL_IDLE: Duration = Duration::from_millis(500);
const MONITOR_MAX_AGE: Duration = Duration::from_millis(500);
pub const HANDOVER_TIMEOUT: Duration = Duration::from_secs(2);
#[derive(Default)]
enum Monitor {
    #[default]
    NotRequired,
    Starting(Instant),
    Ready(Instant),
    Unavailable(&'static str),
}
impl Monitor {
    fn permits_input(&self) -> bool {
        matches!(self, Self::NotRequired)
            || matches!(self, Self::Ready(at) if at.elapsed() <= MONITOR_MAX_AGE)
    }
    fn status(&self) -> serde_json::Value {
        let (state, reason) = match self {
            Self::NotRequired => ("not_enabled", None),
            Self::Starting(_) => ("starting", None),
            Self::Ready(at) if at.elapsed() <= MONITOR_MAX_AGE => ("ready", None),
            Self::Ready(_) => ("unavailable", Some("monitor_unresponsive")),
            Self::Unavailable(reason) => ("unavailable", Some(*reason)),
        };
        json!({"state":state,"reason":reason})
    }
}
#[derive(Default)]
struct State {
    revision: u64,
    paused: bool,
    viewer_held: bool,
    local_held: bool,
    local_state: LocalHeld,
    last_source: Option<&'static str>,
    monitor: Monitor,
    pending_manual: usize,
    last_manual: Option<Instant>,
    active: Option<(String, CancellationToken)>,
}
#[derive(Clone, Default)]
pub(crate) struct Control {
    state: Arc<Mutex<State>>,
    settled: Arc<Notify>,
}
impl Control {
    pub fn revision(&self) -> u64 {
        self.state.lock().unwrap().revision
    }
    pub fn status(&self) -> serde_json::Value {
        status(&self.state.lock().unwrap())
    }
    pub fn monitor_status(&self) -> serde_json::Value {
        self.state.lock().unwrap().monitor.status()
    }
    pub fn monitor_ready(&self) -> bool {
        matches!(self.state.lock().unwrap().monitor, Monitor::Ready(at) if at.elapsed() <= MONITOR_MAX_AGE)
    }
    pub fn can_mutate(&self) -> bool {
        let state = self.state.lock().unwrap();
        !state.paused && state.monitor.permits_input()
    }
    pub fn require_monitor(&self) {
        self.state.lock().unwrap().monitor = Monitor::Starting(Instant::now());
    }
    pub fn monitor_started(&self, held: bool) {
        let mut state = self.state.lock().unwrap();
        let Monitor::Starting(at) = state.monitor else {
            return;
        };
        if at.elapsed() > HANDOVER_TIMEOUT {
            state.monitor = Monitor::Unavailable("startup_timeout");
            pause(&mut state, "monitor");
            return;
        }
        state.monitor = Monitor::Ready(Instant::now());
        state.local_held = held;
        if held {
            pause(&mut state, "local");
        }
    }
    pub fn monitor_heartbeat(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        // Once interrupted, do not silently recover across a lost-input interval.
        if let Monitor::Ready(at) = state.monitor {
            if at.elapsed() > MONITOR_MAX_AGE {
                state.monitor = Monitor::Unavailable("monitor_unresponsive");
                pause(&mut state, "monitor");
            } else {
                state.monitor = Monitor::Ready(Instant::now());
                return true;
            }
        }
        false
    }
    pub fn monitor_failed(&self, reason: &'static str) {
        let mut state = self.state.lock().unwrap();
        if matches!(state.monitor, Monitor::Unavailable(_)) {
            return;
        }
        state.monitor = Monitor::Unavailable(reason);
        pause(&mut state, "monitor");
    }
    pub fn expire_monitor(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if matches!(state.monitor, Monitor::Ready(at) if at.elapsed() > MONITOR_MAX_AGE) {
            state.monitor = Monitor::Unavailable("monitor_unresponsive");
            pause(&mut state, "monitor");
        }
        if matches!(state.monitor, Monitor::Starting(at) if at.elapsed() > HANDOVER_TIMEOUT) {
            state.monitor = Monitor::Unavailable("startup_timeout");
            pause(&mut state, "monitor");
        }
        matches!(state.monitor, Monitor::Unavailable(_))
    }
    /// A passive listener cannot hold back physical input. Stop remaining agent
    /// work immediately; never synthesize, suppress, or log the user's event.
    pub fn local_input(&self, held: bool) {
        let mut state = self.state.lock().unwrap();
        state.local_held = held;
        pause(&mut state, "local");
    }
    pub fn local_state(&self) -> LocalHeld {
        self.state.lock().unwrap().local_state
    }
    pub fn observe_local(&self, input: LocalHeld) {
        let mut state = self.state.lock().unwrap();
        state.local_held = input.held();
        state.local_state = input;
        pause(&mut state, "local");
    }
    pub fn resume(&self, revision: u64) -> Outcome {
        let mut state = self.state.lock().unwrap();
        if state.revision != revision {
            return Err(changed());
        }
        if !state.monitor.permits_input() {
            return Err(monitor_unavailable());
        }
        if state.active.is_some()
            || state.pending_manual != 0
            || state.viewer_held
            || state.local_held
            || state
                .last_manual
                .is_some_and(|at| at.elapsed() < MANUAL_IDLE)
        {
            return Err(Failure::new(
                "manual_busy",
                "Wait for human input to finish before explicitly resuming automation",
            ));
        }
        if state.paused {
            state.paused = false;
            state.revision += 1;
        }
        Ok(status(&state))
    }
    pub fn begin(
        &self,
        operation: &str,
        revision: u64,
        cancel: CancellationToken,
    ) -> Result<Active, Failure> {
        let mut state = self.state.lock().unwrap();
        if state.revision != revision {
            return Err(changed());
        }
        if !state.monitor.permits_input() {
            return Err(monitor_unavailable());
        }
        if state.paused {
            return Err(Failure::new(
                "manual_control",
                "Human input paused automation; query control_status and explicitly resume_control",
            ));
        }
        state.active = Some((operation.into(), cancel));
        Ok(Active {
            control: self.clone(),
            revision,
        })
    }
    /// Called before injecting viewer input. Pausing and cancelling are atomic
    /// with begin(), and the waiter only completes after the provider's cleanup.
    pub async fn manual_input(&self, held: bool) -> Result<ManualInput, Failure> {
        {
            let mut state = self.state.lock().unwrap();
            state.viewer_held = held;
            state.pending_manual += 1;
            pause(&mut state, "viewer");
        }
        let guard = ManualInput(self.clone());
        tokio::time::timeout(HANDOVER_TIMEOUT, async {
            loop {
                let notified = self.settled.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if self.state.lock().unwrap().active.is_none() {
                    break;
                }
                notified.await;
            }
        })
        .await
        .map_err(|_| Failure::new(
            "handover_timeout",
            "Native action has not stopped; close the session instead of mixing human and agent input",
        ))?;
        Ok(guard)
    }
}
fn status(state: &State) -> serde_json::Value {
    json!({"mode":if state.paused{"manual"}else{"automation"},"revision":state.revision,
        "active_operation":state.active.as_ref().map(|(id,_)|id),"input_held":state.viewer_held || state.local_held,
        "viewer_input_held":state.viewer_held,"local_input_held":state.local_held,"last_input_source":state.last_source,
        "local_input_monitor":state.monitor.status(),
        "input_pending":state.pending_manual != 0,
        "can_resume":state.paused && !state.viewer_held && !state.local_held && state.monitor.permits_input() && state.pending_manual == 0 && state.active.is_none() && state.last_manual.is_none_or(|at|at.elapsed()>=MANUAL_IDLE)})
}
fn pause(state: &mut State, source: &'static str) {
    state.paused = true;
    state.revision += 1;
    state.last_manual = Some(Instant::now());
    state.last_source = Some(source);
    if let Some((_, cancel)) = &state.active {
        cancel.cancel();
    }
}
fn monitor_unavailable() -> Failure {
    Failure::new(
        "local_monitor_unavailable",
        "Local input monitoring is unavailable; inspect control_status, resolve the reported cause on the host, then reconnect",
    )
}
/// Keep this guard until the viewer input has actually been dispatched.
/// Idle time starts after dispatch, never while waiting for the agent to stop.
#[must_use]
pub struct ManualInput(Control);
impl std::fmt::Debug for ManualInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ManualInput")
    }
}
impl Drop for ManualInput {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap();
        state.pending_manual -= 1;
        state.last_manual = Some(Instant::now());
    }
}
pub(crate) fn changed() -> Failure {
    Failure::new(
        "control_changed",
        "Control changed for this session; inspect control_status before issuing new actions",
    )
}
pub(crate) struct Active {
    control: Control,
    revision: u64,
}
impl Active {
    pub fn interrupted(&self) -> bool {
        self.control.revision() != self.revision
    }
}
impl Drop for Active {
    fn drop(&mut self) {
        self.control.state.lock().unwrap().active = None;
        self.control.settled.notify_waiters();
    }
}

#[cfg(test)]
mod tests;
