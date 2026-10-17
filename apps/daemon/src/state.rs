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
    /// Set only by the validated system-agent entry point, never by IPC/settings.
    pub login_window: bool,
    pub paths: DataPaths,
    pub settings: Mutex<Settings>,
    pub power: Mutex<crate::power::IdleSleepAssertion>,
    /// Local certificate short fingerprint (computed at startup, used by Status snapshots).
    pub fp_short: String,
    /// Controlled-service switch (whether the host runner should be online).
    pub enabled: AtomicBool,
    pub host_ready: AtomicBool,
    pub host_error: Mutex<Option<String>>,
    pub relay_state: Mutex<(Option<bool>, Option<String>)>,
    /// Notification of `enabled` changes (subscribed by the host manager).
    pub enabled_watch: watch::Sender<bool>,
    pub sessions: Mutex<Vec<SessionInfo>>,
    pub pending_pin: Mutex<Option<String>>,
    pending_pin_expiry: AtomicU64,
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
        let mut power = crate::power::IdleSleepAssertion::default();
        power.set_enabled(settings.prevent_idle_sleep);
        let relay_configured = paths.root.join("relay-host.toml").exists();
        let (events, _) = broadcast::channel(64);
        let (enabled_watch, _) = watch::channel(enabled);
        Self {
            login_window: false,
            paths,
            settings: Mutex::new(settings),
            power: Mutex::new(power),
            fp_short,
            enabled: AtomicBool::new(enabled),
            host_ready: AtomicBool::new(false),
            host_error: Mutex::new(None),
            relay_state: Mutex::new((relay_configured.then_some(false), None)),
            enabled_watch,
            sessions: Mutex::new(Vec::new()),
            pending_pin: Mutex::new(None),
            pending_pin_expiry: AtomicU64::new(0),
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
        let relay = self.relay_state.lock().unwrap().clone();
        let enabled = self.enabled.load(Ordering::SeqCst);
        let ready = enabled && self.host_ready.load(Ordering::SeqCst);
        let power = self.power.lock().unwrap();
        StatusReport {
            prevent_idle_sleep: settings.prevent_idle_sleep,
            idle_sleep_prevented: power.is_active(),
            power_error: power.error().map(str::to_owned),
            running: enabled,
            host_ready: ready,
            host_error: enabled
                .then(|| self.host_error.lock().unwrap().clone())
                .flatten(),
            relay_connected: relay.0.map(|connected| connected && ready),
            relay_error: relay.1,
            port: settings.host_port,
            device_name: settings.device_name.clone(),
            fp_short: self.fp_short.clone(),
            sessions: self.sessions.lock().unwrap().clone(),
            pending_pin: self.active_pin(),
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
        let mut next = Settings::update(&self.paths, |s| s.host_enabled = on)?;
        if self.login_window {
            crate::login_window::restrict(&mut next);
        }
        self.apply_settings(&mut settings, next);
        Ok(())
    }

    /// Reconcile all persisted host settings, including the service switch and
    /// listener configuration. The same lock order as set_enabled prevents a
    /// concurrent reload from restoring an older in-memory snapshot.
    pub fn reload_settings(&self) -> removent_core::Result<()> {
        let mut current = self.settings.lock().unwrap();
        let mut next = Settings::load_strict(&self.paths)?;
        next.authentication
            .validate()
            .map_err(removent_core::CoreError::Serialization)?;
        if self.login_window {
            crate::login_window::restrict(&mut next);
        }
        self.apply_settings(&mut current, next);
        Ok(())
    }

    fn apply_settings(&self, current: &mut Settings, next: Settings) {
        self.power
            .lock()
            .unwrap()
            .set_enabled(next.prevent_idle_sleep);
        let restart = removent_core::settings::control::host_changed(current, &next);
        let enabled = next.host_enabled;
        let policy_changed = current.authentication != next.authentication
            || current.admission != next.admission
            || current.paired_only != next.paired_only;
        *current = next;
        let changed = self.enabled.swap(enabled, Ordering::SeqCst) != enabled;
        if restart || changed {
            self.enabled_watch.send_replace(enabled);
        }
        if changed {
            self.broadcast(IpcEvent::StateChanged { running: enabled });
        }
        if policy_changed || !enabled {
            if let Err(error) = removent_core::pairing_invitation::Invitation::revoke(&self.paths) {
                tracing::warn!(%error, "failed to revoke invitation after reloading settings");
            }
            self.clear_pin();
            removent_host::session::clear_resume_registry();
        }
    }

    /// Pairing PIN display: stash and broadcast it for management ends to present.
    pub fn active_pin(&self) -> Option<String> {
        if !self.enabled.load(Ordering::SeqCst) {
            return None;
        }
        if let Some(pin) = self.pending_pin.lock().unwrap().clone()
            && pin.len() == 6
            && self.pending_pin_expiry.load(Ordering::SeqCst)
                > removent_core::pairing_invitation::now()
        {
            return Some(pin);
        }
        removent_core::pairing_invitation::Invitation::load(&self.paths)
            .ok()
            .flatten()
            .map(|i| i.code().expose().to_owned())
    }
    pub fn clear_pin(&self) {
        *self.pending_pin.lock().unwrap() = None;
        // A persisted invitation may have been displayed from a status snapshot
        // after daemon restart, without a corresponding in-memory pending PIN.
        self.broadcast(IpcEvent::PairingCleared);
    }
    pub fn clear_reactive_pin(&self) {
        let mut pin = self.pending_pin.lock().unwrap();
        if pin.as_ref().is_some_and(|p| p.len() == 6) {
            *pin = None;
            drop(pin);
            self.broadcast(IpcEvent::PairingCleared);
        }
    }
    pub fn show_pin(&self, pin: String) {
        self.pending_pin_expiry.store(
            removent_core::pairing_invitation::now() + 300,
            Ordering::SeqCst,
        );
        *self.pending_pin.lock().unwrap() = Some(pin.clone());
        self.broadcast(IpcEvent::PairingPin { pin });
    }

    pub fn pairing_done(&self, peer_name: String) {
        *self.pending_pin.lock().unwrap() = None;
        self.broadcast(IpcEvent::PairingDone { peer_name });
    }

    /// Session start: register the runner's unique id and broadcast.
    pub fn session_started(&self, id: u64, peer_name: String, peer_fp16: String, codec: String) {
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
        self.broadcast(IpcEvent::SessionStarted { session });
    }

    /// Session end: remove only the matching protocol session and broadcast.
    pub fn session_ended(&self, id: u64, reason: String) {
        let mut sessions = self.sessions.lock().unwrap();
        let count = sessions.len();
        sessions.retain(|s| s.id != id);
        if sessions.len() == count {
            return;
        }
        drop(sessions);
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
