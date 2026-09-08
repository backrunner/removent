//! Shared daemon state: enable switch, session table, pairing PIN, admission
//! arbitration, and event broadcast.

use removent_core::ipc::{IpcEvent, SessionInfo, StatusReport};
use removent_core::{DataPaths, Settings};
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{broadcast, oneshot, watch};
use tokio_util::sync::CancellationToken;

/// Default admission request timeout (protocol §4.4: no reply within 30s counts as denied).
pub const DEFAULT_ADMISSION_TIMEOUT: Duration = Duration::from_secs(30);

pub struct DaemonState {
    pub paths: DataPaths,
    pub settings: Mutex<Settings>,
    /// Local certificate short fingerprint (computed at startup, used by Status snapshots).
    pub fp_short: String,
    /// Controlled-service switch (whether the host runner should be online).
    pub enabled: AtomicBool,
    /// Notification of `enabled` changes (subscribed by the host manager).
    pub enabled_watch: watch::Sender<bool>,
    pub sessions: Mutex<Vec<SessionInfo>>,
    next_session_id: AtomicU64,
    /// ID of the current active session (the runner handles connections serially, at
    /// most one; 0 = none).
    current_session: AtomicU64,
    pub pending_pin: Mutex<Option<String>>,
    /// Number of connected management ends (tray/app/cli).
    pub tray_connections: AtomicUsize,
    pub events: broadcast::Sender<IpcEvent>,
    pending_admissions: Mutex<HashMap<u64, oneshot::Sender<bool>>>,
    next_request_id: AtomicU64,
    pub admission_timeout: Duration,
    /// Master shutdown token (Shutdown request / signal).
    pub shutdown: CancellationToken,
}

/// Also resolves the prompt when an outer timeout or service shutdown drops
/// request_admission before its own timeout can run.
struct PendingAdmission<'a> {
    state: &'a DaemonState,
    request_id: u64,
}

impl Drop for PendingAdmission<'_> {
    fn drop(&mut self) {
        if self
            .state
            .pending_admissions
            .lock()
            .unwrap()
            .remove(&self.request_id)
            .is_some()
        {
            self.state.broadcast(IpcEvent::AdmissionResolved {
                request_id: self.request_id,
                allow: false,
            });
        }
    }
}

impl DaemonState {
    pub fn new(paths: DataPaths, settings: Settings, fp_short: String) -> Self {
        let enabled = settings.host_enabled;
        let (events, _) = broadcast::channel(64);
        let (enabled_watch, _) = watch::channel(enabled);
        Self {
            paths,
            settings: Mutex::new(settings),
            fp_short,
            enabled: AtomicBool::new(enabled),
            enabled_watch,
            sessions: Mutex::new(Vec::new()),
            next_session_id: AtomicU64::new(1),
            current_session: AtomicU64::new(0),
            pending_pin: Mutex::new(None),
            tray_connections: AtomicUsize::new(0),
            events,
            pending_admissions: Mutex::new(HashMap::new()),
            next_request_id: AtomicU64::new(1),
            admission_timeout: DEFAULT_ADMISSION_TIMEOUT,
            shutdown: CancellationToken::new(),
        }
    }

    pub fn broadcast(&self, ev: IpcEvent) {
        // send returns Err when there are no subscribers; that is normal, ignore it.
        let _ = self.events.send(ev);
    }

    /// Status snapshot.
    pub fn snapshot(&self) -> StatusReport {
        let settings = self.settings.lock().unwrap();
        StatusReport {
            running: self.enabled.load(Ordering::SeqCst),
            port: settings.host_port,
            device_name: settings.device_name.clone(),
            fp_short: self.fp_short.clone(),
            sessions: self.sessions.lock().unwrap().clone(),
            pending_pin: self.pending_pin.lock().unwrap().clone(),
            tray_connected: self.tray_connections.load(Ordering::SeqCst) > 0,
            screen_recording_granted: crate::tcc::screen_recording_granted(),
            accessibility_granted: crate::tcc::accessibility_granted(),
        }
    }

    /// Toggle the controlled-service switch and broadcast StateChanged (the host
    /// manager starts/stops the runner via the watch channel).
    pub fn set_enabled(&self, on: bool) -> removent_core::Result<()> {
        // Save before acknowledging so a failed write cannot re-enable remote
        // access after a reboot. Preserve settings changed by another client.
        let mut settings = self.settings.lock().unwrap();
        *settings = Settings::update(&self.paths, |s| s.host_enabled = on)?;
        if self.enabled.swap(on, Ordering::SeqCst) != on {
            self.enabled_watch.send_replace(on);
            self.broadcast(IpcEvent::StateChanged { running: on });
        }
        Ok(())
    }

    /// Pairing PIN display: stash and broadcast it for management ends to present.
    pub fn show_pin(&self, pin: String) {
        *self.pending_pin.lock().unwrap() = Some(pin.clone());
        self.broadcast(IpcEvent::PairingPin { pin });
    }

    pub fn pairing_done(&self, peer_name: String) {
        *self.pending_pin.lock().unwrap() = None;
        self.broadcast(IpcEvent::PairingDone { peer_name });
    }

    /// Session start: register and broadcast, returning the session id.
    pub fn session_started(&self, peer_name: String, peer_fp16: String, codec: String) -> u64 {
        let id = self.next_session_id.fetch_add(1, Ordering::SeqCst);
        let since_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let session = SessionInfo {
            id,
            peer_name,
            peer_fp16,
            since_unix,
            video_codec: codec,
        };
        self.sessions.lock().unwrap().push(session.clone());
        self.current_session.store(id, Ordering::SeqCst);
        self.broadcast(IpcEvent::SessionStarted { session });
        id
    }

    /// Session end: remove the current session and broadcast.
    pub fn session_ended(&self, reason: String) {
        let id = self.current_session.swap(0, Ordering::SeqCst);
        if id == 0 {
            return;
        }
        self.sessions.lock().unwrap().retain(|s| s.id != id);
        self.broadcast(IpcEvent::SessionEnded {
            session_id: id,
            reason,
        });
    }

    /// Start admission arbitration: broadcast AdmissionRequest and asynchronously
    /// await the reply; timeout or no connected management end → false.
    pub async fn request_admission(&self, peer_name: String, peer_fp16: String) -> bool {
        let request_id = self.next_request_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending_admissions
            .lock()
            .unwrap()
            .insert(request_id, tx);
        let _pending = PendingAdmission {
            state: self,
            request_id,
        };
        self.broadcast(IpcEvent::AdmissionRequest {
            request_id,
            peer_name,
            peer_fp16,
        });
        match tokio::time::timeout(self.admission_timeout, rx).await {
            Ok(Ok(allow)) => allow,
            _ => false,
        }
    }

    /// Deliver a management-end AdmissionReply; returns whether the pending request existed.
    pub fn reply_admission(&self, request_id: u64, allow: bool) -> bool {
        let tx = self.pending_admissions.lock().unwrap().remove(&request_id);
        match tx {
            Some(tx) => {
                let _ = tx.send(allow);
                self.broadcast(IpcEvent::AdmissionResolved { request_id, allow });
                true
            }
            None => false,
        }
    }
}
