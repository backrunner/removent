//! Engine bridge: daemon link and client session management on a tokio runtime;
//! the UI receives events over a std channel.
//!
//! The host (controlled) service is carried by a separate daemon (removentd);
//! the app queries and controls it over UDS IPC (core::ipc).

use anyhow::Result;
use futures::FutureExt;
use removent_client::connect_session;
use removent_client::connection::{ConnectionProgress, ConnectionStage};
use removent_client::connection::{ConnectionProtocol, ConnectionRequest};
use removent_client::saved::{SavedConnection, SavedConnections};
use removent_core::ipc::{IpcEvent, IpcRequest, IpcResponse, StatusReport};
use removent_core::{DataPaths, DeviceIdentity, PeersStore, Settings};
use removent_input as rinput;
use removent_net::{PinState, make_client_endpoint};
use removent_proto::{Caps, ControlMsg};
use rust_i18n::t;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tracing::Instrument;

use crate::audio::AudioPlayer;
use crate::updater::{self, UpdateShared, UpdateStatus};

/// One decoded BGRA frame (consumed by UI rendering).
pub use removent_client::DecodedFrame as VideoFrame;

/// UI events (engine → interface).
#[derive(Debug)]
pub enum UiEvent {
    DeviceFound {
        fp: String,
        name: String,
        addr: SocketAddr,
    },
    DeviceLost(String),
    /// Controlled side: display the pairing PIN.
    PairingPin(String),
    /// Pairing complete.
    PairingDone(String),
    /// Controlled side: admission request (forwarded by the daemon), awaiting user decision.
    AdmissionRequest {
        request_id: u64,
        peer_name: String,
        peer_fp_short: String,
    },
    /// Controlling side: the user must enter the peer's PIN to finish pairing.
    ClientNeedsPin {
        generation: usize,
        tx: tokio::sync::oneshot::Sender<String>,
    },
    ConnectionProgress {
        generation: usize,
        stage: ConnectionStage,
    },
    SessionReady {
        generation: usize,
        codec: String,
    },
    /// Controlling-side connect failure (connect button resets + red error;
    /// no SessionClosed will follow).
    ConnectFailed {
        generation: usize,
        error: String,
    },
    /// Controlling-side session ended (including manual disconnect).
    SessionClosed {
        generation: usize,
        reason: String,
    },
    /// Controlled side: a peer connected to this machine (status hint only;
    /// does not affect the controlling-side connecting state).
    HostSessionStarted {
        peer_name: String,
        codec: String,
    },
    /// Controlled side: the peer session ended.
    HostSessionEnded(String),
    /// The admission request has been decided (user click or daemon timeout auto-deny);
    /// the UI closes the dialog accordingly.
    AdmissionResolved {
        request_id: u64,
        allow: bool,
    },
    /// General warning/notice (shown in the status bar), e.g. daemon spawn failure,
    /// device discovery unavailable.
    Notice(String),
    /// Host service running state (daemon Status/StateChanged).
    HostStateChanged(bool),
    /// daemon IPC connection state.
    DaemonOnline(bool),
    /// daemon-process TCC permission snapshot (from StatusReport; changes only —
    /// the daemon preflights at poll time and can hold stale grants after the user
    /// just toggled a permission for the app).
    DaemonPermissions {
        screen_recording: bool,
        accessibility: bool,
    },
    /// Auto-update state machine snapshot (release.md §3; crates/app/src/updater.rs).
    UpdateStatus(UpdateStatus),
}

impl UiEvent {
    pub fn belongs_to_client(&self, current: usize) -> bool {
        match self {
            Self::ClientNeedsPin { generation, .. }
            | Self::ConnectionProgress { generation, .. }
            | Self::SessionReady { generation, .. }
            | Self::ConnectFailed { generation, .. }
            | Self::SessionClosed { generation, .. } => *generation == current,
            _ => true,
        }
    }
}

/// Generation and channel publication share a lock: aborting a tokio task alone
/// does not prevent its current poll from publishing after a cancellation.
#[derive(Default)]
struct ClientChannels {
    generation: usize,
    geometry: Option<(u32, u32)>,
    frames: Option<removent_core::latest::Receiver<VideoFrame>>,
    cmd: Option<ClientCommands>,
    codec: String,
}

enum ClientCommands {
    Standard(tokio::sync::mpsc::Sender<ControlMsg>),
    Vnc(
        removent_client::vnc::InputSender,
        Arc<removent_client::vnc::VncStats>,
    ),
}

impl From<tokio::sync::mpsc::Sender<ControlMsg>> for ClientCommands {
    fn from(tx: tokio::sync::mpsc::Sender<ControlMsg>) -> Self {
        Self::Standard(tx)
    }
}

impl ClientCommands {
    fn try_send(
        &self,
        message: ControlMsg,
    ) -> Result<(), tokio::sync::mpsc::error::TrySendError<ControlMsg>> {
        match self {
            Self::Standard(tx) => enqueue_input(tx, message),
            Self::Vnc(tx, _) => tx.try_send(message),
        }
    }
}

#[derive(Default)]
pub struct ClientDiagnostics {
    pub codec: String,
    pub input: Option<removent_client::vnc::InputSnapshot>,
    pub vnc: Option<removent_client::vnc::VncSnapshot>,
}

impl ClientChannels {
    fn invalidate(&mut self) -> usize {
        self.generation += 1;
        self.frames = None;
        self.geometry = None;
        self.cmd = None;
        self.codec.clear();
        self.generation
    }
}

#[derive(Clone)]
struct ClientAttempt {
    generation: usize,
    started: std::time::Instant,
    channels: Arc<Mutex<ClientChannels>>,
    events: std::sync::mpsc::Sender<UiEvent>,
}

impl ClientAttempt {
    fn progress(&self, stage: ConnectionStage) {
        if self.channels.lock().unwrap().generation == self.generation {
            tracing::info!(
                attempt = self.generation,
                ?stage,
                elapsed_ms = self.started.elapsed().as_millis() as u64,
                "connection progress"
            );
            let _ = self.events.send(UiEvent::ConnectionProgress {
                generation: self.generation,
                stage,
            });
        }
    }
    fn progress_sink(&self) -> ConnectionProgress {
        let attempt = self.clone();
        Arc::new(move |stage| attempt.progress(stage))
    }

    fn publish(
        &self,
        cmd: impl Into<ClientCommands>,
        codec: String,
    ) -> Result<removent_core::latest::Sender<VideoFrame>> {
        let mut channels = self.channels.lock().unwrap();
        anyhow::ensure!(
            channels.generation == self.generation,
            "Connection cancelled"
        );
        let (tx, rx) = removent_core::latest::channel();
        channels.frames = Some(rx);
        channels.geometry = None;
        channels.cmd = Some(cmd.into());
        channels.codec = codec.clone();
        tracing::info!(attempt = self.generation, elapsed_ms = self.started.elapsed().as_millis() as u64, %codec, "remote session ready");
        let _ = self.events.send(UiEvent::SessionReady {
            generation: self.generation,
            codec,
        });
        Ok(tx)
    }

    fn pause_input(&self) {
        let mut channels = self.channels.lock().unwrap();
        if channels.generation == self.generation {
            channels.cmd = None;
        }
    }

    fn resume(&self, cmd: tokio::sync::mpsc::Sender<ControlMsg>) -> Result<()> {
        let mut channels = self.channels.lock().unwrap();
        anyhow::ensure!(
            channels.generation == self.generation,
            "Connection cancelled"
        );
        channels.geometry = None;
        channels.cmd = Some(cmd.into());
        Ok(())
    }

    fn finish(&self, error: Option<String>) {
        let mut channels = self.channels.lock().unwrap();
        if channels.generation != self.generation {
            return;
        }
        channels.cmd = None;
        let event = match error {
            Some(error) => {
                tracing::warn!(attempt = self.generation, elapsed_ms = self.started.elapsed().as_millis() as u64, %error, "connection or session failed");
                UiEvent::ConnectFailed {
                    generation: self.generation,
                    error,
                }
            }
            None => {
                tracing::info!(
                    attempt = self.generation,
                    elapsed_ms = self.started.elapsed().as_millis() as u64,
                    "remote session closed"
                );
                UiEvent::SessionClosed {
                    generation: self.generation,
                    reason: t!("session.peer_disconnected").to_string(),
                }
            }
        };
        let _ = self.events.send(event);
    }
}

type ReqTx = tokio::sync::mpsc::UnboundedSender<IpcRequest>;

pub struct Engine {
    pub(crate) rt: Arc<tokio::runtime::Runtime>,
    paths: DataPaths,
    settings: Arc<Mutex<Settings>>,
    identity: Arc<Mutex<Option<DeviceIdentity>>>,
    pub events_tx: std::sync::mpsc::Sender<UiEvent>,
    /// Event receiver end (shared; can only be taken once, but a clone can still take it).
    pub events_rx: Arc<Mutex<Option<std::sync::mpsc::Receiver<UiEvent>>>>,
    client_task: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    /// Live client-session control-message egress (Some while a session runs);
    /// viewer input and clipboard-clear sends go through here.
    client_channels: Arc<Mutex<ClientChannels>>,
    /// daemon IPC request outlet (Some while online).
    daemon_req: Arc<Mutex<Option<ReqTx>>>,
    daemon_online: Arc<AtomicBool>,
    host_running: Arc<AtomicBool>,
    /// Last daemon-reported TCC permission snapshot (None until the first StatusReport).
    daemon_perms: Arc<Mutex<Option<(bool, bool)>>>,
    /// Auto-update shared state (status + manifest + staged bundle).
    update: Arc<Mutex<UpdateShared>>,
    /// Live controlled-side sessions (daemon IpcEvent SessionStarted/Ended,
    /// reconciled against StatusReport.sessions on every poll);
    /// an update install is deferred while any session runs.
    host_sessions: Arc<AtomicUsize>,
}

impl Clone for Engine {
    fn clone(&self) -> Self {
        Self {
            rt: self.rt.clone(),
            paths: self.paths.clone(),
            settings: self.settings.clone(),
            identity: self.identity.clone(),
            events_tx: self.events_tx.clone(),
            events_rx: self.events_rx.clone(),
            client_task: self.client_task.clone(),
            client_channels: self.client_channels.clone(),
            daemon_req: self.daemon_req.clone(),
            daemon_online: self.daemon_online.clone(),
            host_running: self.host_running.clone(),
            daemon_perms: self.daemon_perms.clone(),
            update: self.update.clone(),
            host_sessions: self.host_sessions.clone(),
        }
    }
}

/// Real clipboard bridge (NSPasteboard).
pub struct NsClipboard;

impl removent_core::TextClipboard for NsClipboard {
    fn change_count(&self) -> Result<u64, String> {
        rinput::change_count().map_err(|e| e.to_string())
    }
    fn read(&self) -> Result<String, String> {
        rinput::read_text().map_err(|e| e.to_string())
    }
    fn write(&self, text: &str) -> Result<(), String> {
        rinput::write_text(text).map_err(|e| e.to_string())
    }
}

/// Fingerprint set of paired devices (the known list for TLS pinning, protocol §4.2).
fn known_fingerprints(paths: &DataPaths) -> Vec<[u8; 32]> {
    PeersStore::load(paths)
        .map(|peers| {
            peers
                .all()
                .iter()
                .filter_map(|p| {
                    let bytes = hex::decode(&p.fingerprint).ok()?;
                    <[u8; 32]>::try_from(bytes.as_slice()).ok()
                })
                .collect()
        })
        .unwrap_or_default()
}

impl Engine {
    pub fn new(rt: tokio::runtime::Runtime, paths: DataPaths, settings: Settings) -> Self {
        let engine = Self::with_state(rt, paths, settings);
        engine.spawn_daemon_link();
        engine.spawn_update_scheduler();
        engine
    }

    fn with_state(rt: tokio::runtime::Runtime, paths: DataPaths, settings: Settings) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self {
            rt: Arc::new(rt),
            paths,
            settings: Arc::new(Mutex::new(settings)),
            identity: Arc::new(Mutex::new(None)),
            events_tx: tx,
            events_rx: Arc::new(Mutex::new(Some(rx))),
            client_task: Arc::new(Mutex::new(None)),
            client_channels: Arc::new(Mutex::new(ClientChannels::default())),
            daemon_req: Arc::new(Mutex::new(None)),
            daemon_online: Arc::new(AtomicBool::new(false)),
            host_running: Arc::new(AtomicBool::new(false)),
            daemon_perms: Arc::new(Mutex::new(None)),
            update: UpdateShared::new(),
            host_sessions: Arc::new(AtomicUsize::new(0)),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_viewer_test(
        paths: DataPaths,
        cmd: tokio::sync::mpsc::Sender<ControlMsg>,
    ) -> Self {
        let engine = Self::with_state(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
            paths,
            Settings::default(),
        );
        engine.client_channels.lock().unwrap().cmd = Some(ClientCommands::Standard(cmd));
        engine
    }

    pub fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    /// Update and persist settings; pushes ReloadSettings to the daemon when online.
    /// Returns Err when the save fails so the UI can surface it.
    pub fn update_settings(&self, f: impl FnOnce(&mut Settings)) -> Result<(), String> {
        {
            let mut s = self.settings.lock().unwrap();
            // The tray/CLI may change host_enabled while this window is open.
            *s = Settings::update(&self.paths, f).map_err(|e| {
                tracing::error!(err=%e, "settings save failed");
                e.to_string()
            })?;
        }
        // Reload on the daemon side (the runner picks up the new settings on next restart).
        if let Some(tx) = self.daemon_req.lock().unwrap().as_ref() {
            let _ = tx.send(IpcRequest::ReloadSettings);
        }
        Ok(())
    }

    pub fn data_dir(&self) -> std::path::PathBuf {
        self.paths.root.clone()
    }

    // ---- auto-update (release.md §3) ----

    /// Current update state machine snapshot.
    pub fn update_status(&self) -> UpdateStatus {
        self.update.lock().unwrap().status.clone()
    }

    /// Kick a manifest check (the settings-page button; always runs unless a
    /// download/install is already in flight).
    pub fn check_for_updates(&self) {
        let endpoint = updater::manifest_endpoint(&self.settings.lock().unwrap());
        let shared = self.update.clone();
        let events = self.events_tx.clone();
        self.rt
            .spawn_blocking(move || updater::run_check(endpoint, true, shared, events));
    }

    /// Download and triple-verify the available update (lands in ReadyToInstall).
    pub fn download_update(&self) {
        let paths = self.paths.clone();
        let shared = self.update.clone();
        let events = self.events_tx.clone();
        self.rt
            .spawn_blocking(move || updater::run_download(paths, shared, events));
    }

    /// Swap in the staged update and relaunch. Refused while a session is
    /// running (the user must end it first); on success the process exits.
    pub fn install_update(&self) -> std::result::Result<(), String> {
        if self.session_active() {
            return Err(t!("update.err.session_active").to_string());
        }
        let shared = self.update.clone();
        let events = self.events_tx.clone();
        let daemon_req = self.daemon_req.clone();
        self.rt
            .spawn_blocking(move || updater::run_install(shared, events, daemon_req));
        Ok(())
    }

    /// True while a controlling-side session (viewer open) or a controlled-side
    /// peer session is running — installs are deferred until it ends.
    pub fn session_active(&self) -> bool {
        let client_live = self
            .client_task
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|t| !t.is_finished());
        client_live || self.host_sessions.load(Ordering::SeqCst) > 0
    }

    /// Check schedule (release.md §3.1): once 30s after launch, then every 24h;
    /// skipped when the user disabled update checks.
    fn spawn_update_scheduler(&self) {
        if std::env::var("REMOVENT_NO_UPDATE_CHECK").as_deref() == Ok("1") {
            return;
        }
        let settings = self.settings.clone();
        let shared = self.update.clone();
        let events = self.events_tx.clone();
        self.rt.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            loop {
                let (enabled, endpoint) = {
                    let s = settings.lock().unwrap();
                    (s.update_check_enabled, updater::manifest_endpoint(&s))
                };
                if enabled {
                    let shared = shared.clone();
                    let events = events.clone();
                    tokio::task::spawn_blocking(move || {
                        updater::run_check(endpoint, false, shared, events);
                    });
                }
                tokio::time::sleep(std::time::Duration::from_secs(24 * 60 * 60)).await;
            }
        });
    }

    pub fn device_name(&self) -> String {
        self.settings.lock().unwrap().device_name.clone()
    }

    pub fn identity(&self) -> Result<DeviceIdentity> {
        let mut cached = self.identity.lock().unwrap();
        if let Some(identity) = cached.as_ref() {
            return Ok(identity.clone());
        }
        let identity = removent_core::identity::load_or_create(&self.paths, &self.device_name())?;
        *cached = Some(identity.clone());
        Ok(identity)
    }

    pub fn fingerprint_short(&self) -> String {
        self.identity()
            .map(|id| id.short_fingerprint_hex())
            .unwrap_or_default()
    }

    // ---- saved connections (bookmarks; passwords are never persisted) ----

    /// Snapshot of the saved-connection list, read fresh so edits by another
    /// window or instance are picked up.
    pub fn saved_connections(&self) -> Vec<SavedConnection> {
        SavedConnections::load(&self.paths)
            .map(|s| s.all().to_vec())
            .unwrap_or_default()
    }

    /// Persist a submitted connection form as a reusable bookmark. Same-endpoint
    /// submissions update the existing entry instead of duplicating it.
    pub fn save_connection(&self, request: &ConnectionRequest, name: String) -> Result<()> {
        let mut store = SavedConnections::load(&self.paths)?;
        store.upsert(SavedConnection::from_request(request, name))?;
        Ok(())
    }

    /// Refresh a bookmark's last-used timestamp (direct reconnects from the list).
    pub fn touch_saved_connection(&self, entry: &SavedConnection) {
        if let Ok(mut store) = SavedConnections::load(&self.paths) {
            let _ = store.upsert(entry.clone());
        }
    }

    pub fn remove_saved_connection(&self, id: &str) -> Result<()> {
        let mut store = SavedConnections::load(&self.paths)?;
        store.remove(id)?;
        Ok(())
    }

    /// Short-fingerprint set of trusted devices (the "Paired" marker in the device list).
    pub fn trusted_short_fps(&self) -> std::collections::HashSet<String> {
        PeersStore::load(&self.paths)
            .map(|p| {
                p.all()
                    .iter()
                    .filter(|r| r.trusted)
                    .map(|r| r.short_fp.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    // ---- daemon control ----

    /// Enable/disable the host service; tries to spawn the daemon first when offline.
    pub fn set_host_enabled(&self, on: bool) {
        if let Some(tx) = self.daemon_req.lock().unwrap().as_ref() {
            let _ = tx.send(IpcRequest::SetEnabled { on });
            return;
        }
        // launchctl readiness can take seconds; never wait on the UI thread.
        let engine = self.clone();
        let slot = self.daemon_req.clone();
        let events = self.events_tx.clone();
        self.rt.spawn(async move {
            let result = tokio::task::spawn_blocking(move || engine.spawn_daemon_process()).await;
            if let Err(e) = result.map_err(anyhow::Error::from).and_then(|v| v) {
                let _ = events.send(UiEvent::Notice(
                    t!("notice.daemon_spawn_failed", err = format!("{e:#}")).to_string(),
                ));
                return;
            }
            for _ in 0..25 {
                if let Some(tx) = slot.lock().unwrap().as_ref() {
                    let _ = tx.send(IpcRequest::SetEnabled { on });
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
            let _ = events.send(UiEvent::Notice(
                t!("notice.daemon_start_timeout").to_string(),
            ));
        });
    }

    /// Start IPC/hosting from the saved settings without changing the service
    /// switch. The packaged app uses launchd; source builds retain dev.sh's lifecycle.
    pub fn start_background_daemon(&self) {
        let Some(bin) = Self::daemon_binary() else {
            return;
        };
        if !bin
            .ancestors()
            .any(|p| p.extension().is_some_and(|e| e == "app"))
        {
            return;
        }
        let engine = self.clone();
        self.rt.spawn_blocking(move || {
            if let Err(e) = engine.spawn_daemon_process() {
                let _ = engine.events_tx.send(UiEvent::Notice(
                    t!("notice.daemon_spawn_failed", err = format!("{e:#}")).to_string(),
                ));
            }
        });
    }

    /// Admission decision reply (forwarded to the daemon).
    pub fn answer_admission(&self, request_id: u64, allow: bool) {
        if let Some(tx) = self.daemon_req.lock().unwrap().as_ref() {
            let _ = tx.send(IpcRequest::AdmissionReply { request_id, allow });
        }
    }

    /// Locate the removentd executable: same directory as the current process
    /// (app bundle / debug dir) first, then PATH.
    fn daemon_binary() -> Option<std::path::PathBuf> {
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
        {
            let candidate = dir.join("removentd");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths).find_map(|dir| {
                let candidate = dir.join("removentd");
                candidate.is_file().then_some(candidate)
            })
        })
    }

    fn spawn_daemon_process(&self) -> Result<()> {
        let bin = Self::daemon_binary()
            .ok_or_else(|| anyhow::anyhow!(t!("notice.daemon_binary_missing").to_string()))?;
        #[cfg(target_os = "macos")]
        if bin
            .ancestors()
            .any(|p| p.extension().is_some_and(|e| e == "app"))
        {
            return removent_core::service::Service::new(self.paths.clone(), bin)?.start();
        }
        let mut child = std::process::Command::new(bin)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        // Reap the child when it exits: dropping the handle without wait() would
        // leave a zombie process.
        std::thread::Builder::new()
            .name("removentd-reaper".into())
            .spawn(move || match child.wait() {
                Ok(status) => tracing::info!(?status, "removentd exited"),
                Err(e) => tracing::warn!(err=%e, "removentd wait failed"),
            })
            .expect("spawn removentd reaper");
        tracing::info!("removentd spawned by app");
        Ok(())
    }

    /// Long-lived daemon IPC link: connect, forward events, poll status periodically,
    /// reconnect on drop.
    fn spawn_daemon_link(&self) {
        let paths = self.paths.clone();
        let events = self.events_tx.clone();
        let slot = self.daemon_req.clone();
        let online = self.daemon_online.clone();
        let running = self.host_running.clone();
        let perms = self.daemon_perms.clone();
        let sessions = self.host_sessions.clone();
        self.rt.spawn(async move {
            loop {
                match removent_core::ipc::connect(&paths).await {
                    Ok((mut r, mut w)) => {
                        online.store(true, Ordering::SeqCst);
                        let _ = events.send(UiEvent::DaemonOnline(true));
                        let (req_tx, mut req_rx) =
                            tokio::sync::mpsc::unbounded_channel::<IpcRequest>();
                        *slot.lock().unwrap() = Some(req_tx);
                        // Take a snapshot right after connecting.
                        let _ = removent_core::ipc::write_msg(&mut w, &IpcRequest::Status).await;
                        let mut reader = removent_core::ipc::MessageReader::default();
                        let mut tick = tokio::time::interval(std::time::Duration::from_secs(3));
                        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                        loop {
                            tokio::select! {
                                line = reader.read::<_, serde_json::Value>(&mut r) => {
                                    match line {
                                        Ok(Some(v)) => handle_daemon_message(
                                            v, &events, &running, &perms, &sessions,
                                        ),
                                        Ok(None) => break, // EOF
                                        Err(e) => {
                                            tracing::warn!(err=%e, "daemon ipc read error");
                                            break;
                                        }
                                    }
                                }
                                Some(req) = req_rx.recv() => {
                                    if removent_core::ipc::write_msg(&mut w, &req).await.is_err() {
                                        break;
                                    }
                                }
                                _ = tick.tick() => {
                                    if removent_core::ipc::write_msg(&mut w, &IpcRequest::Status)
                                        .await
                                        .is_err()
                                    {
                                        break;
                                    }
                                }
                            }
                        }
                        slot.lock().unwrap().take();
                        online.store(false, Ordering::SeqCst);
                        running.store(false, Ordering::SeqCst);
                        sessions.store(0, Ordering::SeqCst);
                        let _ = events.send(UiEvent::DaemonOnline(false));
                    }
                    Err(_) => {
                        online.store(false, Ordering::SeqCst);
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        });
    }

    // ---- client sessions ----

    /// Connect to the peer at the given address and enter a viewing session.
    pub fn connect_to(&self, addr: SocketAddr) -> Result<()> {
        self.connect_request(ConnectionRequest::native(addr))
    }

    pub fn connect_request(&self, request: ConnectionRequest) -> Result<()> {
        // Serialize the check, generation change and task installation with disconnect.
        let mut task = self.client_task.lock().unwrap();
        if task.as_ref().is_some_and(|t| !t.is_finished()) {
            anyhow::bail!(t!("err.session_in_progress").to_string());
        }
        let identity = if request.protocol == ConnectionProtocol::Removent {
            Some(self.identity()?)
        } else {
            None
        };
        let paths = self.paths.clone();
        let settings = self.settings.lock().unwrap().clone();
        let attempt = ClientAttempt {
            generation: self.client_channels.lock().unwrap().invalidate(),
            started: std::time::Instant::now(),
            channels: self.client_channels.clone(),
            events: self.events_tx.clone(),
        };
        // Record only the destination and protocol, never the request/credentials.
        let span = tracing::info_span!("client_connection", pid = std::process::id(), attempt = attempt.generation,
            protocol = ?request.protocol, destination = %request.address);
        tracing::info!(parent: &span, "connection requested");
        let handle = self.rt.spawn(
            async move {
                // catch_unwind: even a task panic must reset the UI (the connect button
                // must not get stuck on "connecting…").
                let result = std::panic::AssertUnwindSafe(run_requested_client(
                    identity,
                    request,
                    paths,
                    settings,
                    attempt.clone(),
                ))
                .catch_unwind()
                .await;
                attempt.finish(match result {
                    Ok(Ok(())) => None,
                    Ok(Err(e)) => Some(format!("{e:#}")),
                    Err(_) => Some(t!("session.internal_error").to_string()),
                });
            }
            .instrument(span),
        );
        *task = Some(handle);
        Ok(())
    }

    pub fn client_generation(&self) -> usize {
        self.client_channels.lock().unwrap().generation
    }

    pub fn take_client_frames(&self) -> Option<removent_core::latest::Receiver<VideoFrame>> {
        self.client_channels.lock().unwrap().frames.take()
    }

    /// Closing an old disconnected viewer must not terminate a newer session.
    pub fn disconnect_client_if(&self, generation: usize) {
        self.cancel_client(
            Some(generation),
            t!("session.manually_disconnected").to_string(),
        );
    }

    /// Disconnect the session proactively; only sends SessionClosed when a live task
    /// exists (avoids a bogus event when closing the window).
    pub fn disconnect_client(&self) {
        self.cancel_client(None, t!("session.manually_disconnected").to_string());
    }

    fn cancel_client(&self, expected_generation: Option<usize>, reason: String) {
        let mut task = self.client_task.lock().unwrap();
        let mut channels = self.client_channels.lock().unwrap();
        if expected_generation.is_some_and(|generation| generation != channels.generation) {
            return;
        }
        // Also invalidate already-queued ready/PIN/error events from a finished task.
        let previous = channels.generation;
        let generation = channels.invalidate();
        if let Some(task) = task.take() {
            if !task.is_finished() {
                tracing::info!(
                    attempt = previous,
                    "connection or session cancelled locally"
                );
            }
            task.abort();
            let _ = self
                .events_tx
                .send(UiEvent::SessionClosed { generation, reason });
        }
    }

    // ---- viewer input / clipboard pass-through ----

    pub fn client_diagnostics(&self, generation: usize) -> ClientDiagnostics {
        let channels = self.client_channels.lock().unwrap();
        if channels.generation != generation {
            return ClientDiagnostics::default();
        }
        let mut diagnostics = ClientDiagnostics {
            codec: channels.codec.clone(),
            ..Default::default()
        };
        if let Some(ClientCommands::Vnc(input, stats)) = &channels.cmd {
            diagnostics.input = Some(input.snapshot());
            diagnostics.vnc = Some(stats.snapshot());
        }
        diagnostics
    }

    /// Publish geometry on the same FIFO as input, and resend it after resume.
    pub fn set_frame_geometry(&self, generation: usize, width: u32, height: u32) {
        let failed = {
            let mut channels = self.client_channels.lock().unwrap();
            if generation != channels.generation || channels.geometry == Some((width, height)) {
                return;
            }
            let Some(tx) = &channels.cmd else {
                return;
            };
            match tx.try_send(ControlMsg::FrameGeometry { width, height }) {
                Ok(()) => {
                    channels.geometry = Some((width, height));
                    false
                }
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => true,
                Err(_) => false,
            }
        };
        if failed {
            self.cancel_client(Some(generation), t!("session.input_overloaded").to_string());
        }
    }

    /// Buffer VNC transitions and coalesce motion; other transports reserve FIFO
    /// space for transitions. A hard overflow closes the session explicitly
    /// instead of silently losing a key/button release.
    fn try_send_cmd(&self, msg: ControlMsg) {
        let failed_generation = {
            let channels = self.client_channels.lock().unwrap();
            channels.cmd.as_ref().and_then(|tx| match tx.try_send(msg) {
                // The frame bridge owns disconnect/retry handling. A closed
                // old channel must not abort a reconnect that is starting.
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => Some(channels.generation),
                _ => None,
            })
        };
        if let Some(generation) = failed_generation {
            self.cancel_client(Some(generation), t!("session.input_overloaded").to_string());
        }
    }

    pub fn send_input_mouse(
        &self,
        display_id: u64,
        x_px: f32,
        y_px: f32,
        buttons: u8,
        kind: removent_proto::MouseKind,
    ) {
        self.try_send_cmd(ControlMsg::MouseEvent {
            display_id,
            x_px,
            y_px,
            buttons,
            kind,
        });
    }

    pub fn send_input_key(
        &self,
        vk_code: u16,
        modifiers: removent_proto::KeyModifiers,
        kind: removent_proto::KeyKind,
        unicode: Option<char>,
    ) {
        self.try_send_cmd(ControlMsg::KeyEvent {
            vk_code,
            modifiers,
            kind,
            unicode,
        });
    }

    pub fn send_input_scroll(
        &self,
        display_id: u64,
        dx_mm: f32,
        dy_mm: f32,
        phase: removent_proto::ScrollPhase,
    ) {
        self.try_send_cmd(ControlMsg::ScrollEvent {
            display_id,
            dx_mm,
            dy_mm,
            phase,
        });
    }

    /// Clear the LOCAL clipboard (NSPasteboard). The viewer's focus-loss clearing
    /// wipes the local pasteboard — where the sensitive remote content landed —
    /// and leaves the remote clipboard alone (it may hold the host user's own
    /// content). The clipboard poller skips empty local content, so this never
    /// propagates to the peer.
    pub fn clear_local_clipboard(&self) {
        if let Err(e) = rinput::write_text("") {
            tracing::warn!(err=%e, "local clipboard clear failed");
        }
    }
}

/// Pointer motion may be skipped under pressure; ordered key/button/scroll
/// transitions either enter the FIFO or cause an explicit session shutdown.
fn enqueue_input(
    tx: &tokio::sync::mpsc::Sender<ControlMsg>,
    msg: ControlMsg,
) -> Result<(), tokio::sync::mpsc::error::TrySendError<ControlMsg>> {
    let motion = matches!(
        msg,
        ControlMsg::MouseEvent {
            kind: removent_proto::MouseKind::Moved
                | removent_proto::MouseKind::LeftDragged
                | removent_proto::MouseKind::RightDragged
                | removent_proto::MouseKind::MiddleDragged,
            ..
        }
    );
    if motion && !tx.is_closed() && tx.capacity() <= (tx.max_capacity() / 4).max(1) {
        return Ok(());
    }
    tx.try_send(msg)
}

/// daemon message dispatch: responses update state, events become UiEvent.
fn handle_daemon_message(
    v: serde_json::Value,
    events: &std::sync::mpsc::Sender<UiEvent>,
    running: &Arc<AtomicBool>,
    perms: &Arc<Mutex<Option<(bool, bool)>>>,
    sessions: &Arc<AtomicUsize>,
) {
    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match ty {
        "status" | "ok" | "error" => {
            match serde_json::from_value::<IpcResponse>(v) {
                Ok(IpcResponse::Status(report)) => {
                    apply_status(*report, events, running, perms, sessions)
                }
                // Daemon-side failures (e.g. a rejected request) must reach the UI
                // instead of being silently dropped.
                Ok(IpcResponse::Error { message }) => {
                    let _ = events.send(UiEvent::Notice(message));
                }
                _ => {}
            }
        }
        _ => {
            if let Ok(ev) = serde_json::from_value::<IpcEvent>(v) {
                match ev {
                    IpcEvent::StateChanged { running: on } => {
                        running.store(on, Ordering::SeqCst);
                        let _ = events.send(UiEvent::HostStateChanged(on));
                    }
                    IpcEvent::SessionStarted { session } => {
                        sessions.fetch_add(1, Ordering::SeqCst);
                        let _ = events.send(UiEvent::HostSessionStarted {
                            peer_name: session.peer_name,
                            codec: session.video_codec,
                        });
                    }
                    IpcEvent::SessionEnded { reason, .. } => {
                        let _ = sessions.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                            Some(n.saturating_sub(1))
                        });
                        let _ = events.send(UiEvent::HostSessionEnded(reason));
                    }
                    IpcEvent::AdmissionRequest {
                        request_id,
                        peer_name,
                        peer_fp16,
                    } => {
                        let _ = events.send(UiEvent::AdmissionRequest {
                            request_id,
                            peer_name,
                            peer_fp_short: peer_fp16,
                        });
                    }
                    IpcEvent::AdmissionResolved { request_id, allow } => {
                        let _ = events.send(UiEvent::AdmissionResolved { request_id, allow });
                    }
                    IpcEvent::PairingPin { pin } => {
                        let _ = events.send(UiEvent::PairingPin(pin));
                    }
                    IpcEvent::PairingDone { peer_name } => {
                        let _ = events.send(UiEvent::PairingDone(peer_name));
                    }
                }
            }
        }
    }
}

fn apply_status(
    report: StatusReport,
    events: &std::sync::mpsc::Sender<UiEvent>,
    running: &Arc<AtomicBool>,
    perms: &Arc<Mutex<Option<(bool, bool)>>>,
    sessions: &Arc<AtomicUsize>,
) {
    let prev = running.swap(report.running, Ordering::SeqCst);
    if prev != report.running {
        let _ = events.send(UiEvent::HostStateChanged(report.running));
    }
    // Reconcile the session count with the daemon's authoritative list: the
    // SessionStarted/Ended increments alone drift when the app starts with
    // sessions already running or the UDS link drops and reconnects, and a
    // false "no session" would let an update install kill a live session.
    sessions.store(report.sessions.len(), Ordering::SeqCst);
    // Notify only on change: status is polled every few seconds.
    let snap = (
        report.screen_recording_granted,
        report.accessibility_granted,
    );
    let mut slot = perms.lock().unwrap();
    if *slot != Some(snap) {
        *slot = Some(snap);
        let _ = events.send(UiEvent::DaemonPermissions {
            screen_recording: snap.0,
            accessibility: snap.1,
        });
    }
}

/// Audio forwarding must stop even when the UI aborts the connection task.
struct AudioForwardTask(tokio::task::JoinHandle<()>);
impl Drop for AudioForwardTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn client_bind_addr(peer: SocketAddr) -> SocketAddr {
    if peer.is_ipv6() {
        SocketAddr::from(([0u16; 8], 0))
    } else {
        SocketAddr::from(([0u8; 4], 0))
    }
}

async fn run_client(
    identity: DeviceIdentity,
    addr: SocketAddr,
    paths: DataPaths,
    settings: Settings,
    attempt: ClientAttempt,
) -> Result<()> {
    let known = known_fingerprints(&paths);
    let (ep_client, _pin) = make_client_endpoint(
        client_bind_addr(addr),
        &identity,
        PinState::new(known, true),
    )?;

    let audio_player = match tokio::task::spawn_blocking(AudioPlayer::new).await? {
        Ok(player) => Some(player),
        Err(e) => {
            tracing::warn!(err = %e, "audio output unavailable; negotiating video only");
            None
        }
    };
    let audio_enabled = audio_player.is_some();

    let mk_cfg = || removent_client::ClientConfig {
        device_name: settings.device_name.clone(),
        caps: Caps {
            audio: audio_enabled,
            ..Caps::all()
        },
        local_clip: Some(Arc::new(NsClipboard)),
    };

    // The PIN prompt is deferred until pairing actually starts: a trusted peer
    // never needs one (no popup flash on every connect).
    let pin_attempt = attempt.clone();
    attempt.progress(ConnectionStage::Negotiating);
    let mut session = connect_session(
        ep_client,
        addr,
        &identity,
        mk_cfg(),
        None,
        None,
        Some(Box::new(move |pin_tx| {
            pin_attempt.progress(ConnectionStage::Pairing);
            let _ = pin_attempt.events.send(UiEvent::ClientNeedsPin {
                generation: pin_attempt.generation,
                tx: pin_tx,
            });
        })),
    )
    .await?;

    // Frame bridge: install the channel before telling the UI to open the viewer
    // (eliminates the race of not being able to take rx).
    let ftx = attempt.publish(
        session.cmd_tx.clone(),
        format!("{:?}", session.negotiated.video.codec),
    )?;
    let mut audio_task = audio_player.as_ref().map(|player| {
        let player = player.clone();
        let (_drop_tx, drop_rx) = tokio::sync::mpsc::channel(1);
        let pcm_rx = std::mem::replace(&mut session.decoded_pcm_rx, drop_rx);
        AudioForwardTask(tokio::spawn(async move {
            let mut pcm_rx = pcm_rx;
            while let Some(pcm) = pcm_rx.recv().await {
                player.push(pcm);
            }
        }))
    });
    loop {
        while let Some(frame) = session.decoded_bgra_rx.recv().await {
            if ftx.send(frame).is_err() {
                return Ok(());
            }
        }
        // The frame channel closed: a clean SessionEnd ends here; an abnormal
        // network drop gets one transparent quick-resume attempt (§7.3/§7.4) —
        // on success the session continues without the UI noticing.
        attempt.pause_input();
        if let Some(mut task) = audio_task.take() {
            task.0.abort();
            let _ = (&mut task.0).await;
        }
        if let Some(player) = &audio_player {
            player.clear();
        }
        if !session.was_interrupted() {
            return Ok(());
        }
        let Some(token) = session.current_resume_token() else {
            return Ok(());
        };
        let prev_ack = session.negotiated.clone();
        // A media stream can fail while QUIC/control remain live. Release that
        // session before retrying, or it keeps the host busy across all retries.
        drop(session);
        tracing::info!("connection dropped abnormally; attempting quick resume");
        // The old session's permit on the host is released at the end of its
        // teardown chain, so a resume attempted immediately after a drop can
        // be rejected with Busy. Retry with a short backoff (well inside the
        // 30s resume window) before reporting the session as closed.
        let mut resumed = None;
        for attempt in 0..3 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            }
            let known = known_fingerprints(&paths);
            let (ep, _pin) = make_client_endpoint(
                client_bind_addr(addr),
                &identity,
                PinState::new(known, true),
            )?;
            match tokio::time::timeout(
                std::time::Duration::from_secs(8),
                removent_client::quick_resume(
                    ep,
                    addr,
                    &identity,
                    mk_cfg(),
                    token,
                    prev_ack.clone(),
                ),
            )
            .await
            {
                Ok(Ok(s)) => {
                    resumed = Some(s);
                    break;
                }
                Ok(Err(e)) => {
                    tracing::warn!(err=%e, attempt, "quick resume attempt failed");
                }
                Err(_) => tracing::warn!(attempt, "quick resume attempt timed out"),
            }
        }
        match resumed {
            Some(s) => {
                session = s;
                attempt.resume(session.cmd_tx.clone())?;
                audio_task = audio_player.as_ref().map(|player| {
                    let player = player.clone();
                    let (_drop_tx, drop_rx) = tokio::sync::mpsc::channel(1);
                    let pcm_rx = std::mem::replace(&mut session.decoded_pcm_rx, drop_rx);
                    AudioForwardTask(tokio::spawn(async move {
                        let mut pcm_rx = pcm_rx;
                        while let Some(pcm) = pcm_rx.recv().await {
                            player.push(pcm);
                        }
                    }))
                });
            }
            None => return Ok(()),
        }
    }
}

/// Translate common VNC failures while keeping uncommon protocol details available.
fn vnc_failure_message(error: &removent_client::VncError) -> String {
    use removent_client::VncError;
    match error {
        VncError::Timeout { stage, seconds } => {
            let key = match *stage {
                "TCP connection" => "connection.vnc_tcp",
                "server greeting" => "connection.vnc_greeting",
                "security negotiation" => "connection.vnc_negotiation",
                "authentication challenge" | "authentication result" => {
                    "connection.vnc_authentication"
                }
                "desktop initialization" => "connection.vnc_desktop",
                _ => return error.to_string(),
            };
            t!("connection.vnc_timeout", stage = t!(key), seconds = seconds).to_string()
        }
        VncError::Authentication => t!("connection.vnc_auth_failed").to_string(),
        VncError::Io(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
            t!("connection.vnc_refused").to_string()
        }
        _ => error.to_string(),
    }
}

/// Connect to a standard RFB/VNC server and bridge its raw frames into the
/// same viewer bus used by RVP. VNC has no Removent pairing/resume channel, so
/// a disconnect is reported directly to the UI.
async fn run_vnc_client(request: ConnectionRequest, attempt: ClientAttempt) -> Result<()> {
    let target = request.address.to_string();
    let progress = attempt.progress_sink();
    attempt.progress(ConnectionStage::Resolving);
    let mut session = tokio::time::timeout(std::time::Duration::from_secs(90), async {
        let addresses = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio::net::lookup_host((request.address.host.as_str(), request.address.port)),
        )
        .await
        .map_err(|_| anyhow::anyhow!("VNC host lookup timed out for {target}"))??;
        let mut last_error = anyhow::anyhow!("No addresses found for {target}");
        for addr in addresses {
            match removent_client::vnc::connect_vnc_with_progress(
                addr,
                &request.username,
                &request.password,
                Some(&progress),
            )
            .await
            {
                Ok(session) => return Ok(session),
                Err(e) => {
                    tracing::warn!(%addr, err = %e, "VNC connection failed");
                    // Retrying another DNS address cannot fix account credentials.
                    let authentication_failed =
                        matches!(e, removent_client::VncError::Authentication);
                    last_error = anyhow::anyhow!("{addr}: {}", vnc_failure_message(&e));
                    if authentication_failed {
                        break;
                    }
                }
            }
        }
        Err(last_error)
    })
    .await
    .map_err(|_| {
        anyhow::anyhow!("VNC connection to {target} exceeded 90 seconds during server negotiation")
    })??;
    let ftx = attempt.publish(
        ClientCommands::Vnc(session.cmd_tx.clone(), session.stats.clone()),
        "RFB/VNC · Raw BGRA".into(),
    )?;
    while let Some(frame) = session.decoded_bgra_rx.recv().await {
        if ftx.send(frame).is_err() {
            break;
        }
    }
    Ok(())
}

async fn run_requested_client(
    identity: Option<DeviceIdentity>,
    request: ConnectionRequest,
    paths: DataPaths,
    settings: Settings,
    attempt: ClientAttempt,
) -> Result<()> {
    match request.protocol {
        ConnectionProtocol::Removent => {
            attempt.progress(ConnectionStage::Resolving);
            let addr = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                tokio::net::lookup_host((request.address.host.as_str(), request.address.port)),
            )
            .await??
            .next()
            .ok_or_else(|| anyhow::anyhow!("No addresses found for {}", request.address.host))?;
            run_client(
                identity.ok_or_else(|| anyhow::anyhow!("Device identity missing"))?,
                addr,
                paths,
                settings,
                attempt,
            )
            .await
        }
        ConnectionProtocol::Vnc => run_vnc_client(request, attempt).await,
        ConnectionProtocol::Rdp => {
            let mut session = removent_client::rdp::connect_rdp_with_progress(
                request,
                Some(attempt.progress_sink()),
            )
            .await?;
            let ftx = attempt.publish(session.cmd_tx.clone(), "RDP".into())?;
            while let Some(frame) = session.decoded_bgra_rx.recv().await {
                if ftx.send(frame).is_err() {
                    return Ok(());
                }
            }
            (&mut session.completion)
                .await
                .map_err(|_| anyhow::anyhow!("RDP session task stopped unexpectedly"))?
        }
    }
}

#[cfg(test)]
mod client_lifecycle_tests {
    use super::*;

    fn attempt(
        channels: &Arc<Mutex<ClientChannels>>,
        events: &std::sync::mpsc::Sender<UiEvent>,
    ) -> ClientAttempt {
        ClientAttempt {
            generation: channels.lock().unwrap().invalidate(),
            started: std::time::Instant::now(),
            channels: channels.clone(),
            events: events.clone(),
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
        old.resume(cmd).unwrap();
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
        let frames = old.publish(cmd, "RDP".into()).unwrap();
        let (tx, _) = tokio::sync::oneshot::channel();
        events
            .send(UiEvent::ClientNeedsPin {
                generation: old.generation,
                tx,
            })
            .unwrap();
        old.progress(ConnectionStage::Authenticating);
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
        assert!(old.resume(cmd.clone()).is_err());
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
}
