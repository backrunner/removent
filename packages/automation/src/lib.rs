//! Session-scoped Computer Use executor. Native objects stay on one worker.
use removent_proto::automation::{Command, Failure, Request};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

mod control;
pub use control::ManualInput;
#[cfg(target_os = "macos")]
mod native;
pub type Outcome = Result<Value, Failure>;

pub trait Provider {
    fn execute(&mut self, command: &Command, cancel: &CancellationToken) -> Outcome;
    fn invalidate_observations(&mut self) {}
}

struct Job {
    request: Request,
    revision: u64,
    cancel: CancellationToken,
    reply: oneshot::Sender<Outcome>,
}

#[derive(Clone)]
pub struct Executor {
    tx: mpsc::Sender<Job>,
    control: control::Control,
}

impl Executor {
    /// At most one native worker per host process, including a stalled previous session.
    /// A cancelled native OS call cannot be killed safely; never accumulate threads.
    #[cfg(target_os = "macos")]
    pub fn native(session: CancellationToken) -> Result<Self, Failure> {
        static SLOT: std::sync::OnceLock<std::sync::Arc<tokio::sync::Semaphore>> =
            std::sync::OnceLock::new();
        let permit = std::sync::Arc::new(
            SLOT.get_or_init(|| std::sync::Arc::new(tokio::sync::Semaphore::new(1)))
                .clone()
                .try_acquire_owned()
                .map_err(|_| Failure::new("busy", "Native worker is still stopping"))?,
        );
        let runtime = tokio::runtime::Handle::current();
        let control = control::Control::default();
        control.require_monitor();
        let monitor_control = control.clone();
        let monitor_session = session.clone();
        Self::spawn_control(session, control, move || {
            let monitor = native::monitor::start(
                monitor_control.clone(),
                monitor_session,
                &runtime,
                permit.clone(),
            );
            (
                native::Native::new(runtime, monitor_control),
                (monitor, permit),
            )
        })
    }

    pub fn spawn<P: Provider + 'static, G: 'static>(
        session: CancellationToken,
        factory: impl FnOnce() -> (P, G) + Send + 'static,
    ) -> Result<Self, Failure> {
        Self::spawn_control(session, control::Control::default(), factory)
    }

    fn spawn_control<P: Provider + 'static, G: 'static>(
        session: CancellationToken,
        control: control::Control,
        factory: impl FnOnce() -> (P, G) + Send + 'static,
    ) -> Result<Self, Failure> {
        let (tx, mut rx) = mpsc::channel::<Job>(4);
        let runtime = tokio::runtime::Handle::current();
        let worker_control = control.clone();
        std::thread::Builder::new().name("removent-automation".into()).spawn(move || {
            let (mut provider, _guard) = factory();
            let mut ledger = Ledger::default();
            let mut observed_revision = 0;
            loop {
                let job = runtime.block_on(async {
                    tokio::select! { biased; _ = session.cancelled() => None, job = rx.recv() => job }
                });
                let Some(job) = job else { break };
                let revision = worker_control.revision();
                if observed_revision != revision { provider.invalidate_observations(); observed_revision = revision; }
                let result = ledger.run(&job.request, &job.cancel, || {
                    if let Command::ResumeControl { revision } = job.request.command {
                        return worker_control.resume(revision);
                    }
                    if job.request.command.mutates() {
                        if job.revision != worker_control.revision() { return Err(control::changed()); }
                        let active = worker_control.begin(&job.request.operation_id, job.request.control_revision.unwrap_or(0), job.cancel.clone())?;
                        let outcome = provider.execute(&job.request.command, &job.cancel);
                        if active.interrupted() { Err(control::changed()) } else { outcome }
                    } else {
                        let outcome = provider.execute(&job.request.command, &job.cancel);
                        if worker_control.revision() != revision { Err(control::changed()) } else { outcome }
                    }
                });
                let _ = job.reply.send(result);
            }
        }).map_err(|_| Failure::new("unavailable", "Cannot start native worker"))?;
        Ok(Self { tx, control })
    }

    pub async fn manual_input(&self, held: bool) -> Result<ManualInput, Failure> {
        self.control.manual_input(held).await
    }
    /// Platform listener entry point. This is never exposed as an RVP command.
    pub fn local_input(&self, held: bool) {
        self.control.local_input(held);
    }
    pub async fn execute(&self, request: Request, cancel: CancellationToken) -> Outcome {
        request.validate()?;
        if matches!(request.command, Command::ControlStatus {}) {
            return Ok(self.control.status());
        }
        let revision = self.control.revision();
        let cancel = cancel.child_token();
        let _cancel_on_drop = cancel.clone().drop_guard();
        let (reply, rx) = oneshot::channel();
        self.tx
            .try_send(Job {
                request,
                revision,
                cancel,
                reply,
            })
            .map_err(|_| Failure::new("busy", "Automation queue is full or closed"))?;
        rx.await
            .map_err(|_| Failure::new("session_closed", "Automation session ended"))?
    }
}

#[derive(Default)]
struct Ledger {
    // Never evict mutation IDs during a session: retrying an older ID must not repeat input.
    completed: HashMap<String, ([u8; 32], Outcome)>,
}

impl Ledger {
    fn run(
        &mut self,
        request: &Request,
        cancel: &CancellationToken,
        execute: impl FnOnce() -> Outcome,
    ) -> Outcome {
        request.validate()?;
        let digest: [u8; 32] =
            Sha256::digest(serde_json::to_vec(&request.command).expect("validated command")).into();
        if let Some((previous, outcome)) = self.completed.get(&request.operation_id) {
            return if previous == &digest {
                outcome.clone()
            } else {
                Err(Failure::new(
                    "operation_conflict",
                    "Operation ID was already used with different arguments",
                ))
            };
        }
        if request.command.mutates() && self.completed.len() >= 4096 {
            return Err(Failure::new(
                "session_limit",
                "Reconnect before issuing more operations",
            ));
        }
        let result = check_cancel(cancel).and_then(|_| execute());
        if request.command.mutates() {
            self.completed
                .insert(request.operation_id.clone(), (digest, result.clone()));
        }
        result
    }
}

pub(crate) fn check_cancel(cancel: &CancellationToken) -> Result<(), Failure> {
    if cancel.is_cancelled() {
        Err(Failure::new(
            "cancelled",
            "Operation cancelled; earlier input may have been applied",
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
