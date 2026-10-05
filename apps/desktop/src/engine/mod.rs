//! Engine bridge: daemon link and client session management on a tokio runtime;
//! the UI receives events over a std channel.
//!
//! The host (controlled) service is carried by a separate daemon (removentd);
//! the app queries and controls it over UDS IPC (core::ipc).

use anyhow::Result;
use futures::FutureExt;
use removent_client::connection::{ConnectionProgress, ConnectionStage};
use removent_client::connection::{ConnectionProtocol, ConnectionRequest};
use removent_client::keychain;
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
        protocol: ConnectionProtocol,
    },
    DeviceLost(String),
    /// Controlled side: display the pairing PIN.
    PairingPin(String),
    /// Pairing complete.
    PairingDone(String),
    PairingCleared,
    /// Controlled side: admission request (forwarded by the daemon), awaiting user decision.
    AdmissionRequest {
        request_id: u64,
        peer_name: String,
        peer_fp_short: String,
    },
    /// Controlling side: the user must enter the peer's PIN to finish pairing.
    ClientNeedsPin {
        generation: usize,
        mode: removent_core::AuthenticationMode,
        tx: tokio::sync::oneshot::Sender<String>,
    },
    ConfirmCertificate {
        generation: usize,
        destination: String,
        relay: bool,
        tx: tokio::sync::oneshot::Sender<bool>,
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
    /// Auto-update state machine snapshot (release.md §3; apps/desktop/src/updater.rs).
    UpdateStatus(UpdateStatus),
    PairedConnections {
        generation: usize,
        entries: Vec<SavedConnection>,
    },
    CloudSync {
        status: serde_json::Value,
        entries: Vec<SavedConnection>,
    },
}

impl UiEvent {
    pub fn belongs_to_client(&self, current: usize) -> bool {
        match self {
            Self::PairedConnections { generation, .. }
            | Self::ClientNeedsPin { generation, .. }
            | Self::ConfirmCertificate { generation, .. }
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
    Native(removent_client::InputSender),
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

impl From<removent_client::InputSender> for ClientCommands {
    fn from(tx: removent_client::InputSender) -> Self {
        Self::Native(tx)
    }
}

impl ClientCommands {
    fn try_send(
        &self,
        message: ControlMsg,
    ) -> Result<(), tokio::sync::mpsc::error::TrySendError<ControlMsg>> {
        match self {
            Self::Standard(tx) => enqueue_input(tx, message),
            Self::Native(tx) => tx.try_send(message),
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
    fn certificate_confirmation(
        &self,
        destination: String,
        relay: bool,
    ) -> removent_net::CertificateConfirmation {
        let attempt = self.clone();
        Box::new(move |_, tx| {
            if attempt.channels.lock().unwrap().generation == attempt.generation {
                let _ = attempt.events.send(UiEvent::ConfirmCertificate {
                    generation: attempt.generation,
                    destination,
                    relay,
                    tx,
                });
            }
        })
    }
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

    fn resume(&self, cmd: impl Into<ClientCommands>) -> Result<()> {
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
    discovery_settings: tokio::sync::watch::Sender<removent_core::DiscoverySettings>,
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
            discovery_settings: self.discovery_settings.clone(),
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
        crate::cloud_sync::start(&engine);
        engine
    }

    fn with_state(rt: tokio::runtime::Runtime, paths: DataPaths, settings: Settings) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let (discovery_settings, _) = tokio::sync::watch::channel(settings.discovery);
        let update = UpdateShared::new(&settings);
        Self {
            rt: Arc::new(rt),
            paths,
            settings: Arc::new(Mutex::new(settings)),
            discovery_settings,
            identity: Arc::new(Mutex::new(None)),
            events_tx: tx,
            events_rx: Arc::new(Mutex::new(Some(rx))),
            client_task: Arc::new(Mutex::new(None)),
            client_channels: Arc::new(Mutex::new(ClientChannels::default())),
            daemon_req: Arc::new(Mutex::new(None)),
            daemon_online: Arc::new(AtomicBool::new(false)),
            host_running: Arc::new(AtomicBool::new(false)),
            daemon_perms: Arc::new(Mutex::new(None)),
            update,
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

mod compatibility;
mod connection;
mod daemon;
mod daemon_events;
mod input;
mod native;
/// daemon message dispatch: responses update state, events become UiEvent.
/// Audio forwarding must stop even when the UI aborts the connection task.
/// Translate common VNC failures while keeping uncommon protocol details available.
mod preferences;
mod saved;
#[cfg(test)]
mod tests;

use compatibility::run_requested_client;
use daemon_events::handle_daemon_message;
use native::run_client;
