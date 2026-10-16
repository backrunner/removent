//! Resident host-service runner: QUIC endpoint + mDNS advertisement + accept loop.
//!
//! Reusable logic factored down from the app engine, shared by the daemon (removentd)
//! and the app's embedded mode. Interactions (PIN display / admission ruling / session
//! events) are bridged to the host environment via [`HostCallbacks`].

use anyhow::Result;
use futures::SinkExt;
use removent_core::{DataPaths, PeersStore, Settings, TextClipboard, identity};
use removent_net::{Advertiser, PinState, RvpConnection, make_server_endpoint};
use removent_proto::{Caps, ControlMsg, HandshakeServer, PROTO_VERSION};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

mod connection;
mod display;

use crate::input_sink::InputSink;
use crate::session::PromptFuture;
use crate::vnc::{VncConfig, serve_vnc};
use connection::run_connection;

/// Static runner config (unchanged within one serve_forever call; changing settings
/// requires restarting the runner).
pub struct HostRunnerConfig {
    pub paths: DataPaths,
    pub settings: Settings,
    /// Real machines pass RealInputSink; headless/tests pass None or a recorder.
    pub input_sink: Option<Arc<dyn InputSink>>,
    /// Local clipboard bridge; None disables clipboard sync.
    pub local_clip: Option<Arc<dyn TextClipboard>>,
}

/// runner → host session/pairing events.
#[derive(Debug, Clone)]
pub enum HostEvent {
    Listening,
    RelayState {
        connected: bool,
        error: Option<String>,
    },
    SessionStarted {
        session_id: u64,
        peer_name: String,
        peer_fp16: String,
        codec: String,
    },
    SessionEnded {
        session_id: u64,
        reason: String,
    },
    /// A displayed reactive PIN is no longer usable.
    PairingCleared,
    /// Pairing completed (PIN flow finished; the peer is registered as a trusted device).
    PairingDone {
        peer_name: String,
    },
}

/// Interaction callbacks provided by the host environment (the daemon bridges to IPC,
/// the app bridges to the UI).
pub struct HostCallbacks {
    /// Display the pairing PIN (non-blocking).
    pub show_pairing_pin: Box<dyn Fn(String) + Send + Sync>,
    /// Admission ruling: awaits the result asynchronously (the implementor is
    /// responsible for a timeout fallback).
    pub admission_prompt: Box<dyn Fn(String, String) -> PromptFuture + Send + Sync>,
    pub on_event: Box<dyn Fn(HostEvent) + Send + Sync>,
}

/// Fingerprint set of paired devices (the known list for TLS pinning, protocol §4.2).
fn known_fingerprints(paths: &DataPaths) -> Vec<[u8; 32]> {
    PeersStore::load(paths)
        .map(|peers| {
            peers
                .all()
                .iter()
                .filter(|p| p.trusted)
                .filter_map(|p| {
                    let bytes = hex::decode(&p.fingerprint).ok()?;
                    <[u8; 32]>::try_from(bytes.as_slice()).ok()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Answers a connection received while a session is active with
/// SessionReject{Busy}, so the second client fails fast instead of hanging.
/// The handshake read inside is bounded by the server-side accept timeout, so a
/// stalled busy-path client cannot leak the spawned task either.
pub fn reject_busy(conn: RvpConnection, device_name: String, paths: DataPaths) {
    tokio::spawn(reject_busy_inner(conn, device_name, paths));
}

async fn reject_busy_inner(conn: RvpConnection, device_name: String, _paths: DataPaths) {
    // Busy rejects this attempt without authenticating it. Suppress credentials
    // even on unknown devices or a quick resume with no input callback.
    let hs = HandshakeServer {
        proto_version: PROTO_VERSION,
        // Honest declaration, same as serve_connection.
        feature_bits: 0,
        device_name,
        resume_accepted: None,
        peer_known: true,
    };
    match conn.accept_handshake(|_| hs).await {
        Ok((_hello, mut sink, _source)) => {
            let _ = sink
                .send(ControlMsg::SessionReject {
                    reason: removent_proto::RejectReason::Busy,
                })
                .await;
            // Hold the connection until the peer closes (bounded): an immediate
            // drop sends CONNECTION_CLOSE, which can race ahead of the reject
            // frame and leave the client with a bare "application closed".
            let _ = tokio::time::timeout(Duration::from_secs(2), conn.inner().closed()).await;
        }
        Err(e) => tracing::warn!(err=%e, "busy-reject handshake failed"),
    }
    // The connection closes when dropped.
}

struct CloseConnection(RvpConnection);
impl Drop for CloseConnection {
    fn drop(&mut self) {
        self.0.inner().close(0u32.into(), b"session ended");
    }
}

/// Everything a per-connection task needs (factored out of the accept loop so
/// connections can be served concurrently while the session semaphore keeps at
/// most one real session alive).
struct ConnectionCtx {
    paths: DataPaths,
    settings: Settings,
    identity: removent_core::DeviceIdentity,
    input_sink: Option<Arc<dyn InputSink>>,
    local_clip: Option<Arc<dyn TextClipboard>>,
    cbs: Arc<HostCallbacks>,
    advertiser: Arc<Advertiser>,
    /// Slot for the active session's stop token (read by the shutdown path).
    active_session: Arc<std::sync::Mutex<Option<CancellationToken>>>,
}

/// Resident host service: listen + advertise + spawn a task per connection until
/// stopped.
///
/// Stop semantics: clear `running` and cancel `shutdown`; the accept loop then
/// cancels the active session and waits briefly so the control pump can run its
/// teardown (releasing held keys/buttons) before the endpoint is dropped.
pub async fn serve_forever(
    cfg: HostRunnerConfig,
    cbs: HostCallbacks,
    running: Arc<AtomicBool>,
    shutdown: CancellationToken,
) -> Result<()> {
    let settings = cfg.settings;
    let identity = identity::load_or_create(&cfg.paths, &settings.device_name)?;
    let fp_short = identity.short_fingerprint_hex();
    let port = settings.host_port;
    let known = known_fingerprints(&cfg.paths);
    let (ep_server, _pin) = make_server_endpoint(
        SocketAddr::from(([0, 0, 0, 0], port)),
        &identity,
        PinState::new(known, !settings.paired_only),
    )?;
    let advertiser = Arc::new(
        Advertiser::start(&settings.device_name, &fp_short, port, Caps::all())
            .map_err(|e| anyhow::anyhow!("{e}"))?,
    );
    if settings.authentication.mode == removent_core::AuthenticationMode::PairingCode {
        advertiser.set_pairing(
            removent_core::pairing_invitation::Invitation::load(&cfg.paths)?
                .map(|v| v.advertisement()),
        )?;
    }
    // Each protocol has one slot; RVP and VNC may capture concurrently.
    let slots = ProtocolSlots::default();

    let _shutdown_on_drop = shutdown.clone().drop_guard();
    let mut connections = tokio::task::JoinSet::new();
    let cbs = Arc::new(cbs);

    // Optional legacy RFB listener with independent input geometry, TCP
    // lifecycle and framebuffer capture.
    let vnc_shutdown = shutdown.child_token();
    if settings.vnc_enabled {
        let vnc_cfg = VncConfig {
            bind_addr: SocketAddr::from(([0, 0, 0, 0], settings.vnc_port)),
            password: settings.vnc_password.clone(),
            input_sink: session_input(&cfg.input_sink),
            shutdown: vnc_shutdown.clone(),
            session_slot: slots.vnc.clone(),
            callbacks: Some(cbs.clone()),
        };
        connections.spawn(async move {
            if let Err(e) = serve_vnc(vnc_cfg).await {
                tracing::error!(err=%e, "VNC listener failed");
            }
        });
    }

    // Further RVP connections get SessionReject{Busy}; additional VNC clients
    // are rejected by their own listener's slot.
    let active_session: Arc<std::sync::Mutex<Option<CancellationToken>>> =
        Arc::new(std::sync::Mutex::new(None));

    (cbs.on_event)(HostEvent::Listening);
    // The bridge belongs to the host runner, never to desktop/tray lifetime.
    let relay_config = cfg.paths.root.join("relay-host.toml");
    if relay_config.exists() {
        let identity = identity.clone();
        let invitation_paths = cfg.paths.clone();
        let pairing_enabled =
            settings.authentication.mode == removent_core::AuthenticationMode::PairingCode;
        let cbs = cbs.clone();
        let stop = shutdown.clone();
        connections.spawn(async move {
            let mut delay = 1;
            loop {
                let result = async {
                    let cfg = removent_relay::config::TunnelConfig::load(&relay_config)?;
                    let tunnel = removent_relay::client::connect_host_with_invitation(
                        &cfg,
                        &identity,
                        if pairing_enabled {
                            removent_core::pairing_invitation::Invitation::load(&invitation_paths)?
                                .map(|v| v.advertisement())
                        } else {
                            None
                        },
                    )
                    .await?;
                    (cbs.on_event)(HostEvent::RelayState {
                        connected: true,
                        error: None,
                    });
                    tunnel.run(([127, 0, 0, 1], port).into()).await
                };
                tokio::select! {
                    _ = stop.cancelled() => break,
                    result = result => {
                        // Library errors never contain credential values.
                        let error = result.err().map(|e| e.to_string());
                        (cbs.on_event)(HostEvent::RelayState { connected: false, error });
                    }
                }
                tokio::select! {
                    _ = stop.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_secs(delay)) => {},
                }
                delay = (delay * 2).min(30);
            }
        });
    }
    // Accept connections until the service is stopped.
    loop {
        if !running.load(Ordering::SeqCst) {
            break;
        }
        let incoming = tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = connections.join_next(), if !connections.is_empty() => continue,
            incoming = ep_server.accept() => incoming,
        };
        let Some(incoming) = incoming else {
            break;
        };
        // Bound handshake/busy-reject tasks as well as established sessions.
        // A slow TLS peer must not serialize acceptance of healthy peers.
        if connections.len() >= 16 {
            incoming.refuse();
            continue;
        }
        let ctx = ConnectionCtx {
            paths: cfg.paths.clone(),
            settings: settings.clone(),
            identity: identity.clone(),
            input_sink: session_input(&cfg.input_sink),
            local_clip: cfg.local_clip.clone(),
            cbs: cbs.clone(),
            advertiser: advertiser.clone(),
            active_session: active_session.clone(),
        };
        let stop = shutdown.clone();
        let slot = slots.native.clone();
        connections.spawn(async move {
            let work = async move {
                let quinn_conn = match incoming.await {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!(err=%e, "connection handshake failed");
                        return;
                    }
                };
                let conn = RvpConnection::new(quinn_conn);
                let _close = CloseConnection(conn.clone());
                let Ok(permit) = slot.try_acquire_owned() else {
                    reject_busy_inner(conn, ctx.settings.device_name, ctx.paths).await;
                    return;
                };
                run_connection(ctx, conn, permit).await;
            };
            tokio::select! {
                biased;
                _ = stop.cancelled() => {},
                _ = work => {},
            }
        });
    }

    // Cancellation reaches pairing/admission as well as media sessions.
    shutdown.cancel();
    ep_server.close(0u32.into(), b"host stopped");
    vnc_shutdown.cancel();
    if tokio::time::timeout(Duration::from_secs(1), async {
        while connections.join_next().await.is_some() {}
    })
    .await
    .is_err()
    {
        connections.shutdown().await;
    }
    drop(advertiser);
    Ok(())
}

pub(crate) struct SessionEventGuard {
    id: u64,
    callbacks: Arc<HostCallbacks>,
}

impl SessionEventGuard {
    pub(crate) fn new(
        callbacks: Arc<HostCallbacks>,
        peer_name: String,
        peer_fp16: String,
        codec: String,
    ) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        (callbacks.on_event)(HostEvent::SessionStarted {
            session_id: id,
            peer_name,
            peer_fp16,
            codec,
        });
        Self { id, callbacks }
    }
}

impl Drop for SessionEventGuard {
    fn drop(&mut self) {
        (self.callbacks.on_event)(HostEvent::SessionEnded {
            session_id: self.id,
            reason: "peer disconnected".into(),
        });
    }
}

struct ProtocolSlots {
    native: Arc<tokio::sync::Semaphore>,
    vnc: Arc<tokio::sync::Semaphore>,
}

impl Default for ProtocolSlots {
    fn default() -> Self {
        Self {
            native: Arc::new(tokio::sync::Semaphore::new(1)),
            vnc: Arc::new(tokio::sync::Semaphore::new(1)),
        }
    }
}

fn session_input(input: &Option<Arc<dyn InputSink>>) -> Option<Arc<dyn InputSink>> {
    input
        .as_ref()
        .map(|sink| sink.for_session().unwrap_or_else(|| sink.clone()))
}

#[cfg(test)]
mod tests;
