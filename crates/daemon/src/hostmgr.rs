//! Host runner lifecycle management: start/stop serve_forever following the
//! `enabled` switch.

use removent_core::TextClipboard;
use removent_host::{HostCallbacks, HostEvent, HostRunnerConfig, RealInputSink, serve_forever};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use crate::state::DaemonState;

/// Real clipboard bridge (NSPasteboard), used by the runner's clipboard sync.
struct NsClipboard;

impl TextClipboard for NsClipboard {
    fn change_count(&self) -> Result<u64, String> {
        removent_input::change_count().map_err(|e| e.to_string())
    }
    fn read(&self) -> Result<String, String> {
        removent_input::read_text().map_err(|e| e.to_string())
    }
    fn write(&self, text: &str) -> Result<(), String> {
        removent_input::write_text(text).map_err(|e| e.to_string())
    }
}

/// Runner callbacks → IPC event bridge.
fn build_callbacks(state: &Arc<DaemonState>) -> HostCallbacks {
    let st_pin = state.clone();
    let st_adm = state.clone();
    let st_ev = state.clone();
    HostCallbacks {
        show_pairing_pin: Box::new(move |pin| st_pin.show_pin(pin)),
        admission_prompt: Box::new(move |peer_name, fp16| {
            let st = st_adm.clone();
            Box::pin(async move { st.request_admission(peer_name, fp16).await })
        }),
        on_event: Box::new(move |ev| match ev {
            HostEvent::SessionStarted {
                peer_name,
                peer_fp16,
                codec,
            } => {
                st_ev.session_started(peer_name, peer_fp16, codec);
            }
            HostEvent::SessionEnded { reason } => st_ev.session_ended(reason),
            HostEvent::PairingDone { peer_name } => st_ev.pairing_done(peer_name),
        }),
    }
}

/// Run/stop the controlled service following `enabled`, until master shutdown.
///
/// Stop semantics: clear the running flag and cancel the runner's shutdown
/// token; serve_forever then cancels the active session and waits briefly so
/// the control pump can inject release events for held keys/buttons (aborting
/// the task directly would skip that teardown and leave stuck input). The
/// endpoint and mDNS Advertiser go offline when the runner returns, and are
/// rebuilt wholesale on re-enable.
pub async fn run(state: Arc<DaemonState>) {
    let mut enabled_rx = state.enabled_watch.subscribe();
    loop {
        if state.shutdown.is_cancelled() {
            break;
        }
        if !state.enabled.load(Ordering::SeqCst) {
            tokio::select! {
                _ = state.shutdown.cancelled() => break,
                changed = enabled_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    continue;
                }
            }
        }

        let settings = state.settings.lock().unwrap().clone();
        let cfg = HostRunnerConfig {
            paths: state.paths.clone(),
            settings,
            input_sink: Some(Arc::new(RealInputSink::new())),
            local_clip: Some(Arc::new(NsClipboard)),
        };
        let running = Arc::new(AtomicBool::new(true));
        let runner_shutdown = CancellationToken::new();
        let mut task = tokio::spawn(serve_forever(
            cfg,
            build_callbacks(&state),
            running.clone(),
            runner_shutdown.clone(),
        ));

        tokio::select! {
            _ = state.shutdown.cancelled() => {
                stop_runner(&running, &runner_shutdown, &mut task).await;
                break;
            }
            changed = enabled_rx.changed() => {
                // Switch flipped (or watch closed): stop the runner and loop back to re-evaluate.
                stop_runner(&running, &runner_shutdown, &mut task).await;
                if changed.is_err() {
                    break;
                }
            }
            res = &mut task => {
                match res {
                    Ok(Ok(())) => tracing::info!("host runner exited"),
                    Ok(Err(e)) => tracing::error!(err=%format!("{e:#}"), "host runner failed"),
                    Err(e) => tracing::error!(err=%e, "host runner panicked"),
                }
                // Abnormal exit with the switch still on: restart after a delay to
                // avoid a crash storm.
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

/// Graceful runner stop: flag + shutdown token, then wait for the runner to
/// finish session teardown; abort only as the last resort.
async fn stop_runner(
    running: &AtomicBool,
    runner_shutdown: &CancellationToken,
    task: &mut tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    running.store(false, Ordering::SeqCst);
    runner_shutdown.cancel();
    if tokio::time::timeout(Duration::from_secs(2), &mut *task)
        .await
        .is_err()
    {
        tracing::warn!("host runner did not stop in time; aborting");
        task.abort();
        let _ = task.await;
    }
}
