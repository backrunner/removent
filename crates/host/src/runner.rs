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

use crate::input_sink::InputSink;
use crate::session::{
    ControlPumpDeps, HostConfig, HostInteractions, PromptFuture, fit_capture_dims,
    serve_connection, spawn_audio_loop, spawn_control_pump, spawn_video_loop,
};
use crate::vnc::{VncConfig, serve_vnc};

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
    SessionStarted {
        peer_name: String,
        peer_fp16: String,
        codec: String,
    },
    SessionEnded {
        reason: String,
    },
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

async fn reject_busy_inner(conn: RvpConnection, device_name: String, paths: DataPaths) {
    // Honest pairing hint, computed like serve_connection does: a trusted peer
    // hitting a busy host must not see a PIN-popup flash.
    let peer_known = conn
        .peer_fingerprint()
        .map(hex::encode)
        .and_then(|fp| {
            PeersStore::load(&paths)
                .ok()
                .map(|peers| peers.by_fingerprint(&fp).is_some())
        })
        .unwrap_or(false);
    let hs = HandshakeServer {
        proto_version: PROTO_VERSION,
        // Honest declaration, same as serve_connection.
        feature_bits: 0,
        device_name,
        resume_accepted: None,
        peer_known,
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

/// Resets the mDNS busy bit when the session task exits (any path).
struct BusyGuard(Arc<Advertiser>);

impl Drop for BusyGuard {
    fn drop(&mut self) {
        if let Err(e) = self.0.set_busy(false) {
            tracing::warn!(err=%e, "mDNS busy-bit reset failed");
        }
    }
}

/// Dropping a JoinHandle detaches it; session children must instead be aborted.
#[derive(Default)]
struct SessionTasks(Vec<tokio::task::JoinHandle<()>>);

impl Drop for SessionTasks {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

struct SessionExit {
    cancel: CancellationToken,
    active: Arc<std::sync::Mutex<Option<CancellationToken>>>,
    cbs: Arc<HostCallbacks>,
}

impl Drop for SessionExit {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.active.lock().unwrap().take();
        (self.cbs.on_event)(HostEvent::SessionEnded {
            reason: "peer disconnected".into(),
        });
    }
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
        PinState::new(known, true),
    )?;
    let advertiser = Arc::new(
        Advertiser::start(&settings.device_name, &fp_short, port, Caps::all())
            .map_err(|e| anyhow::anyhow!("{e}"))?,
    );
    // At most one active session across both RVP and legacy VNC. Both paths
    // share the same screen capture and input injection resources.
    let session_slot = Arc::new(tokio::sync::Semaphore::new(1));

    let _shutdown_on_drop = shutdown.clone().drop_guard();
    let mut connections = tokio::task::JoinSet::new();

    // Optional legacy RFB listener. It shares the host's input sink but has an
    // independent TCP lifecycle and framebuffer capture per VNC client.
    let vnc_shutdown = shutdown.child_token();
    if settings.vnc_enabled {
        let vnc_cfg = VncConfig {
            bind_addr: SocketAddr::from(([0, 0, 0, 0], settings.vnc_port)),
            password: settings.vnc_password.clone(),
            input_sink: cfg.input_sink.clone(),
            shutdown: vnc_shutdown.clone(),
            session_slot: session_slot.clone(),
        };
        connections.spawn(async move {
            if let Err(e) = serve_vnc(vnc_cfg).await {
                tracing::error!(err=%e, "VNC listener failed");
            }
        });
    }

    let cbs = Arc::new(cbs);
    // Further RVP connections get SessionReject{Busy}; VNC connections are
    // rejected at accept time while the same semaphore is held.
    let active_session: Arc<std::sync::Mutex<Option<CancellationToken>>> =
        Arc::new(std::sync::Mutex::new(None));

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
            input_sink: cfg.input_sink.clone(),
            local_clip: cfg.local_clip.clone(),
            cbs: cbs.clone(),
            advertiser: advertiser.clone(),
            active_session: active_session.clone(),
        };
        let stop = shutdown.clone();
        let slot = session_slot.clone();
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

/// Serves one accepted connection: negotiation/pairing, then the media session.
/// Holds the session semaphore permit until the session ends.
async fn run_connection(
    ctx: ConnectionCtx,
    conn: RvpConnection,
    _permit: tokio::sync::OwnedSemaphorePermit,
) {
    // Reload peers per connection: pairing/grant changes take effect immediately.
    let mut peers = match PeersStore::load(&ctx.paths) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(err=%e, "peers store load failed");
            return;
        }
    };
    let peer_fp = conn.peer_fingerprint().map(hex::encode).unwrap_or_default();
    let was_known = !peer_fp.is_empty() && peers.by_fingerprint(&peer_fp).is_some();

    let cb_pin = ctx.cbs.clone();
    let cb_adm = ctx.cbs.clone();
    let interactions = HostInteractions {
        show_pairing_pin: Box::new(move |pin| (cb_pin.show_pairing_pin)(pin)),
        admission_prompt: Box::new(move |peer_name, fp16| {
            (cb_adm.admission_prompt)(peer_name, fp16)
        }),
    };

    let display =
        removent_input::display_list()
            .into_iter()
            .next()
            .unwrap_or(removent_proto::DisplayInfo {
                id: 1,
                w_px: 1280,
                h_px: 720,
                scale: 1.0,
                dpi: 96,
                is_main: true,
            });

    let host_cfg = HostConfig {
        device_name: ctx.settings.device_name.clone(),
        admission: ctx.settings.admission,
        video_bitrate_kbps: 8_000,
        video_fps: 60,
        input_sink: ctx.input_sink.clone(),
        local_clip: ctx.local_clip.clone(),
    };

    let served = serve_connection(
        conn.clone(),
        &ctx.identity,
        &mut peers,
        &host_cfg,
        interactions,
        display.clone(),
    )
    .await;

    let (established, kf_rx, quality_rx, cmd_rx, sink, source) = match served {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(err=%e, "connection closed before session established");
            return;
        }
    };

    let peer_name = peers
        .by_fingerprint(&established.peer_fp_hex)
        .map(|p| p.name.clone())
        .unwrap_or_else(|| "unknown device".to_string());
    if !was_known {
        (ctx.cbs.on_event)(HostEvent::PairingDone {
            peer_name: peer_name.clone(),
        });
    }
    (ctx.cbs.on_event)(HostEvent::SessionStarted {
        peer_name,
        peer_fp16: established.peer_fp_hex.chars().take(16).collect(),
        codec: format!("{:?}", established.ack.video.codec),
    });

    // Advertise the busy bit while the session lives (reset on any exit path).
    if let Err(e) = ctx.advertiser.set_busy(true) {
        tracing::warn!(err=%e, "mDNS busy-bit set failed");
    }
    let _busy_guard = BusyGuard(ctx.advertiser.clone());
    // Expose the session stop token for the graceful-shutdown path.
    *ctx.active_session.lock().unwrap() = Some(established.cancel.clone());
    let _exit = SessionExit {
        cancel: established.cancel.clone(),
        active: ctx.active_session.clone(),
        cbs: ctx.cbs.clone(),
    };
    let mut tasks = SessionTasks::default();

    // Warn once if input injection would silently fail (Accessibility TCC).
    if host_cfg.input_sink.is_some() && !removent_input::accessibility_trusted() {
        tracing::warn!(
            "accessibility permission not granted; input injection will fail (System Settings > Privacy & Security > Accessibility)"
        );
    }

    // The control pump samples actual delivery and owns the complete quality state.
    let delivery = Arc::new(crate::delivery::DeliveryHealth::new(conn.clone()));

    // Control pump: input injection / clipboard application / adaptive delivery
    // (trimmed by the peer's capabilities).
    let deps = ControlPumpDeps {
        kf_tx: Some(established.keyframe_req_tx.clone()),
        controller: Some(established.controller.clone()),
        window_ms: 250,
        input: host_cfg.input_sink.clone(),
        local_clip: host_cfg.local_clip.clone(),
        quality_tx: Some(established.quality_tx.clone()),
        caps: established.peer_caps,
        clip_state: established.clip_state.clone(),
        cancel: established.cancel.clone(),
        peer_fp: Some(established.peer_fp_hex.clone()),
        delivery: Some(delivery.clone()),
    };
    tasks.0.push(spawn_control_pump(source, sink, deps, cmd_rx));

    // Media loops + SCK capture. The capture is aspect-fit into the 1920×1080
    // bounding box (independent clamping would stretch e.g. 16:10 panels).
    let (video_tx, video_rx) = removent_core::latest::channel::<(Vec<u8>, i64)>();
    let (audio_tx, audio_rx) = tokio::sync::mpsc::channel::<removent_media_capture::AudioFrame>(32);
    let (cap_w, cap_h) = fit_capture_dims(display.w_px, display.h_px);
    let (w, h) = (cap_w as usize, cap_h as usize);
    // Tell the input sink which frame dimensions the peer's coordinates refer
    // to, so it can rescale capture px → display physical px before injection.
    if let Some(sink) = host_cfg.input_sink.as_ref() {
        sink.set_capture_dims(cap_w, cap_h);
    }
    let vstream = match conn.open_media_stream().await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(err=%e, "open video stream failed");
            established.cancel.cancel();
            return;
        }
    };
    tasks.0.push(spawn_video_loop(
        vstream,
        video_rx,
        kf_rx,
        quality_rx,
        established.ack.video.codec,
        w,
        h,
        established.ack.video.max_bitrate_kbps,
        established.ack.video.max_fps,
        established.cancel.clone(),
        // Fatal encoder errors end the session explicitly (client is notified).
        Some(established.cmd_tx.clone()),
        display.id,
        Some(delivery),
        host_cfg.input_sink.clone(),
    ));
    // Audio follows the negotiation result: a peer that declined audio gets no
    // audio stream, no encode loop, and no audio capture (§5.2).
    let audio_enabled = established.ack.audio.enabled;
    if audio_enabled {
        let astream = match conn.open_media_stream().await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(err=%e, "open audio stream failed");
                established.cancel.cancel();
                return;
            }
        };
        tasks.0.push(spawn_audio_loop(
            astream,
            audio_rx,
            established.ack.audio.bitrate_kbps,
            established.cancel.clone(),
        ));
    }

    let mut cap = removent_media_capture::start_display_capture(
        display.id as u32,
        cap_w,
        cap_h,
        video_tx,
        audio_enabled.then_some(audio_tx),
    );
    match &mut cap {
        Ok(cap) => {
            // The capture stream may stop on its own (SCK error, display
            // reconfiguration): end the session explicitly instead of leaving
            // the client on a frozen frame.
            if let Some(mut stopped_rx) = cap.take_stopped_rx() {
                let cmd_tx = established.cmd_tx.clone();
                let cancel = established.cancel.clone();
                tasks.0.push(tokio::spawn(async move {
                    if let Some(reason) = stopped_rx.recv().await {
                        tracing::error!(%reason, "capture stream stopped unexpectedly");
                        let _ = cmd_tx
                            .send(ControlMsg::SessionEnd {
                                reason: removent_proto::EndReason::InternalError,
                            })
                            .await;
                        let _ =
                            tokio::time::timeout(Duration::from_secs(1), cancel.cancelled()).await;
                        cancel.cancel();
                    }
                }));
            }
        }
        Err(e) => {
            tracing::error!(err=%e, "SCK capture failed (permission or environment)");
            // Without capture the client would stare at a black screen forever; end the
            // session explicitly (same SessionEnd{InternalError} reporting as the
            // encoder-fatal path in spawn_video_loop) and stop the media loops.
            let _ = established
                .cmd_tx
                .send(ControlMsg::SessionEnd {
                    reason: removent_proto::EndReason::InternalError,
                })
                .await;
            let _ =
                tokio::time::timeout(Duration::from_secs(1), established.cancel.cancelled()).await;
            established.cancel.cancel();
        }
    }

    established.cancel.cancelled().await;
    drop(cap);
    // Release input before returning the session permit. Drop handles safely
    // on early return or cancellation as well.
    tasks.0[0].abort();
    let _ = (&mut tasks.0[0]).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use removent_proto::{HandshakeClient, Hello};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_reaps_connection_waiting_for_pairing() {
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths {
            root: dir.path().to_owned(),
        };
        let id = identity::load_or_create(&paths, "shutdown-test").unwrap();
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = socket.local_addr().unwrap();
        drop(socket);
        let stop = CancellationToken::new();
        // Callbacks are retained by ConnectionCtx: a leaked negotiation task
        // would keep this sentinel alive after serve_forever returns.
        let sentinel = Arc::new(());
        let weak = Arc::downgrade(&sentinel);
        let cfg = HostRunnerConfig {
            paths,
            settings: Settings {
                host_port: addr.port(),
                vnc_enabled: false,
                ..Settings::default()
            },
            input_sink: None,
            local_clip: None,
        };
        let mut task = tokio::spawn(serve_forever(
            cfg,
            HostCallbacks {
                show_pairing_pin: Box::new(|_| {}),
                admission_prompt: Box::new(|_, _| Box::pin(async { true })),
                on_event: Box::new(move |_| {
                    let _keep = &sentinel;
                }),
            },
            Arc::new(AtomicBool::new(true)),
            stop.clone(),
        ));
        let (client, _) = removent_net::make_client_endpoint(
            "127.0.0.1:0".parse().unwrap(),
            &id,
            PinState::new([], true),
        )
        .unwrap();
        let conn = tokio::time::timeout(
            Duration::from_secs(5),
            client.connect(addr, "removent").unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
        let conn = RvpConnection::new(conn);
        let _control = conn
            .connect_handshake(HandshakeClient {
                magic: removent_proto::MAGIC,
                proto_version: PROTO_VERSION,
                feature_bits: 0,
                hello: Hello {
                    app_version: "test".into(),
                    device_name: "test".into(),
                    os_version: "test".into(),
                    caps: Caps::all(),
                    resume_token: None,
                },
            })
            .await
            .unwrap();
        // No pairing stream arrives. Previously this task survived host stop
        // for the full pairing deadline and retained the advertiser/resources.
        stop.cancel();
        tokio::time::timeout(Duration::from_secs(2), &mut task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(
            weak.upgrade().is_none(),
            "negotiation task retained runner callbacks"
        );
        tokio::time::timeout(Duration::from_secs(1), conn.inner().closed())
            .await
            .unwrap();
    }
}
