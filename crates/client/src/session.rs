//! Controller-side session engine: connect, pairing, negotiation, media receive, and
//! stats pump.
//!
//! Sequencing mirrors the host engine (protocol.md §4–5); decoded media output is
//! delivered over channels to the render layer or a test collector. Fast reconnect
//! recovery: [`quick_resume`].

use crate::jitter::{AudioPacketIn, JitterBuffer, PopOutcome};
use futures::{SinkExt, StreamExt};
use removent_core::{ClipSyncState, DeviceIdentity};
use removent_media_codec::{AudioDecoder, VideoDecoder, extract_param_sets};
use removent_net::{
    ControlItem, ControlSink, ControlSource, PairingMsg, RvpConnection, client_begin,
    client_confirm_check, client_verify,
};
use removent_proto::{
    Caps, ControlMsg, EndReason, HandshakeClient, Hello, NegotiateAck, parse_audio_header,
    parse_video_header,
};

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

#[derive(Debug, Error)]
pub enum ConnectError {
    #[error("net: {0}")]
    Net(#[from] removent_net::NetError),
    #[error("timeout waiting for {0}")]
    Timeout(&'static str),
    #[error("rejected: {0}")]
    Rejected(String),
    #[error("pairing failed: {0}")]
    Pairing(String),
}

/// Connection configuration.
pub struct ClientConfig {
    pub device_name: String,
    pub caps: Caps,
    /// Local clipboard bridge (NSPasteboard on real machines; an in-memory impl in tests).
    pub local_clip: Option<std::sync::Arc<dyn removent_core::TextClipboard>>,
}

/// User input callback: returns the PIN transcribed by the user.
pub type PinInput = oneshot::Receiver<String>;

/// Fired only when the session actually needs a PIN (unknown peer, pairing
/// initiated): the UI shows the PIN prompt at this point, not at connect time.
/// The callback receives the sender half used to deliver the transcribed PIN.
pub type PinRequest = Box<dyn FnOnce(oneshot::Sender<String>) + Send>;

/// Minimum interval between KeyframeRequests (protocol.md §7.1).
const KEYFRAME_MIN_INTERVAL: Duration = Duration::from_millis(500);

/// A decoded BGRA frame (consumed by the renderer; carries frame dimensions to support
/// resolution hot-switching).
#[derive(Debug, Clone)]
pub struct DecodedFrame {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub pts_us: i64,
}

/// An established client session.
pub struct ClientSession {
    pub conn: RvpConnection,
    /// App layer → peer control-message egress (mouse/keyboard/clipboard/keyframe/end).
    pub cmd_tx: mpsc::Sender<ControlMsg>,
    pub negotiated: NegotiateAck,
    /// Most recently received quick-resume token (the host rotates it every session, §7.4).
    pub resume_token: Arc<Mutex<Option<[u8; 16]>>>,
    /// Decoded BGRA frames (consumed by the renderer).
    pub decoded_bgra_rx: mpsc::Receiver<DecodedFrame>,
    /// Decoded PCM (consumed by playback).
    pub decoded_pcm_rx: mpsc::Receiver<Vec<i16>>,
    /// Most recent host quality directive (bitrate_kbps, fps, scale).
    pub last_quality: Arc<Mutex<Option<(u32, u8, f32)>>>,
    /// Time of the last outbound KeyframeRequest (rate limiting).
    last_kf: Arc<Mutex<Option<Instant>>>,
    /// Background task handles (aborted on close/drop of the session).
    tasks: Vec<tokio::task::JoinHandle<()>>,
    /// Media sub-loop handles (registered dynamically by dispatch; aborted on close/drop).
    media_tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

impl ClientSession {
    /// Request a keyframe (quality convergence / stall recovery); rate-limited to a
    /// 500ms minimum interval (protocol.md §7.1).
    pub async fn request_keyframe(&self) -> Result<(), ConnectError> {
        {
            let mut last = self.last_kf.lock().unwrap();
            let now = Instant::now();
            if last.is_some_and(|t| now.duration_since(t) < KEYFRAME_MIN_INTERVAL) {
                return Ok(());
            }
            *last = Some(now);
        }
        self.send(ControlMsg::KeyframeRequest).await
    }

    /// Current quick-resume token (if any). Consume it with [`quick_resume`] when reconnecting.
    pub fn current_resume_token(&self) -> Option<[u8; 16]> {
        *self.resume_token.lock().unwrap()
    }

    /// Proactive close: send SessionEnd, then terminate all background tasks and close
    /// the connection.
    pub async fn close(mut self) -> Result<(), ConnectError> {
        let _ = self
            .send(ControlMsg::SessionEnd {
                reason: EndReason::ClientClosed,
            })
            .await;
        self.teardown();
        Ok(())
    }

    fn teardown(&mut self) {
        for t in self.tasks.drain(..) {
            t.abort();
        }
        for t in self.media_tasks.lock().unwrap().drain(..) {
            t.abort();
        }
        self.conn
            .inner()
            .close(removent_net::quinn::VarInt::from_u32(0), b"session closed");
    }

    pub async fn send_input_mouse(
        &self,
        display_id: u64,
        x_px: f32,
        y_px: f32,
        buttons: u8,
        kind: removent_proto::MouseKind,
    ) -> Result<(), ConnectError> {
        self.send(ControlMsg::MouseEvent {
            display_id,
            x_px,
            y_px,
            buttons,
            kind,
        })
        .await
    }

    pub async fn send_input_key(
        &self,
        vk_code: u16,
        modifiers: removent_proto::KeyModifiers,
        kind: removent_proto::KeyKind,
        unicode: Option<char>,
    ) -> Result<(), ConnectError> {
        self.send(ControlMsg::KeyEvent {
            vk_code,
            modifiers,
            kind,
            unicode,
        })
        .await
    }

    pub async fn send_input_scroll(
        &self,
        display_id: u64,
        dx_mm: f32,
        dy_mm: f32,
        phase: removent_proto::ScrollPhase,
    ) -> Result<(), ConnectError> {
        self.send(ControlMsg::ScrollEvent {
            display_id,
            dx_mm,
            dy_mm,
            phase,
        })
        .await
    }

    pub async fn send_clipboard_text(&self, seq: u32, text: &str) -> Result<(), ConnectError> {
        self.send(ControlMsg::ClipboardSync {
            seq,
            format: removent_proto::ClipFormat::TextUtf8,
            data: text.as_bytes().to_vec(),
        })
        .await
    }

    /// Send a stats report (input for adaptive quality adjustment).
    pub async fn send_stats(
        &self,
        rtt_ms: f32,
        loss_pct: f32,
        recv_kbps: u32,
        jitter_ms: f32,
    ) -> Result<(), ConnectError> {
        self.send(ControlMsg::StatsReport {
            rtt_ms,
            loss_pct,
            recv_kbps,
            jitter_ms,
            decode_ms: 0.0,
            render_fps: 0.0,
        })
        .await
    }

    pub async fn end_session(&self) -> Result<(), ConnectError> {
        self.send(ControlMsg::SessionEnd {
            reason: EndReason::ClientClosed,
        })
        .await
    }

    async fn send(&self, msg: ControlMsg) -> Result<(), ConnectError> {
        self.cmd_tx
            .send(msg)
            .await
            .map_err(|_| ConnectError::Rejected("session closed".into()))
    }
}

impl Drop for ClientSession {
    fn drop(&mut self) {
        self.teardown();
    }
}

/// Default negotiation wait timeout.
const NEGOTIATE_TIMEOUT: Duration = Duration::from_secs(15);
/// SessionAccept wait timeout. On a first connect the host finishes pairing before
/// admitting the session, so the budget must cover the full pairing window (PIN
/// entry, up to 300s) plus the admission prompt (30s) plus margin (protocol.md §4.4).
const ADMISSION_TIMEOUT: Duration = Duration::from_secs(360);

async fn expect_msg(
    source: &mut ControlSource,
    what: &'static str,
    timeout: Duration,
    pred: impl Fn(&ControlMsg) -> bool + Copy,
) -> Result<ControlMsg, ConnectError> {
    loop {
        let item = tokio::time::timeout(timeout, source.next())
            .await
            .map_err(|_| ConnectError::Timeout(what))?
            .transpose()
            .map_err(ConnectError::Net)?
            .ok_or(ConnectError::Timeout(what))?;
        if let ControlItem::Msg(m) = item
            && pred(&m)
        {
            return Ok(*m);
        }
    }
}

/// Establish a full session. When `resume_token` is Some, takes the quick-resume path
/// (no PIN input needed) — but only if the host explicitly accepted the token;
/// a rejected token falls back to full negotiation.
#[allow(clippy::too_many_arguments)]
pub async fn connect_session(
    ep: removent_net::quinn::Endpoint,
    addr: SocketAddr,
    identity: &DeviceIdentity,
    cfg: ClientConfig,
    resume_token: Option<[u8; 16]>,
    prev_ack: Option<NegotiateAck>,
    pin_request: Option<PinRequest>,
) -> Result<ClientSession, ConnectError> {
    // QUIC handshake (briefly retries while the server is not ready).
    let conn = {
        let mut attempt = 0;
        loop {
            match ep.connect(addr, "removent") {
                Ok(connecting) => {
                    let qconn = connecting
                        .await
                        .map_err(|e| ConnectError::Rejected(format!("quinn: {e}")))?;
                    break removent_net::RvpConnection::new(qconn);
                }
                Err(e) => {
                    attempt += 1;
                    if attempt > 50 {
                        return Err(ConnectError::Rejected(format!("connect: {e}")));
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
        }
    };

    let peer_fp_full = conn
        .peer_fingerprint()
        .map(hex_encode)
        .ok_or_else(|| ConnectError::Rejected("peer fingerprint missing".into()))?;

    let (server_ack, sink, mut source) = conn
        .connect_handshake(HandshakeClient {
            magic: removent_proto::MAGIC,
            proto_version: removent_proto::PROTO_VERSION,
            // Honest declaration: file-transfer/two-way-audio are both unimplemented.
            feature_bits: 0,
            hello: Hello {
                app_version: removent_core::APP_VERSION.to_string(),
                device_name: cfg.device_name.clone(),
                os_version: format!("macOS {}", os_version()),
                caps: cfg.caps,
                resume_token,
            },
        })
        .await?;

    // The quick-resume fast path is taken only when the host explicitly accepted
    // the token; a rejected/expired token falls back to full negotiation below.
    let resume_accepted = resume_token.is_some() && server_ack.resume_accepted == Some(true);

    // First connect: start the pairing-initiator task concurrently (the host accepts
    // when needed) — but only when the host does not know us yet; a trusted peer
    // never pairs, so the user is never asked for a PIN (no popup flash).
    // The pairing result comes back over a oneshot, with errors attributed to Pairing
    // rather than a bare Timeout.
    let mut pairing_rx: Option<oneshot::Receiver<Result<(), String>>> = None;
    let mut pairing_task: Option<tokio::task::JoinHandle<()>> = None;
    if !resume_accepted
        && !server_ack.peer_known
        && let Some(pin_request) = pin_request
    {
        let (pin_tx, pin_rx) = oneshot::channel::<String>();
        // Only now is the PIN genuinely needed: ask the UI to prompt the user.
        pin_request(pin_tx);
        let (handle, rx) = spawn_pairing_initiator(
            conn.clone(),
            identity,
            identity.fingerprint_hex(),
            peer_fp_full,
            pin_rx,
        );
        pairing_rx = Some(rx);
        pairing_task = Some(handle);
    }

    let mut sink = sink;
    sink.send(ControlMsg::SessionRequest { caps: cfg.caps })
        .await?;

    // On failure paths, collect the pairing result: briefly wait for the pairing task
    // to produce an outcome (on a wrong PIN the host rejects pairing before
    // disconnecting, so the result is available almost immediately), avoiding the user
    // seeing only a bare Timeout.
    async fn take_pairing_error(
        rx: &mut Option<oneshot::Receiver<Result<(), String>>>,
    ) -> Option<String> {
        let rx = rx.take()?;
        match tokio::time::timeout(Duration::from_millis(500), rx).await {
            Ok(Ok(Err(pe))) => Some(pe),
            _ => None,
        }
    }

    // SessionAccept wait: first-time pairing (up to 300s) + host prompt 30s + margin.
    let resp = match expect_msg(&mut source, "SessionAccept", ADMISSION_TIMEOUT, |m| {
        matches!(
            m,
            ControlMsg::SessionAccept | ControlMsg::SessionReject { .. }
        )
    })
    .await
    {
        Ok(m) => m,
        Err(e) => {
            // Prefer surfacing the pairing failure reason (wrong PIN, etc.) over
            // reporting a bare Timeout.
            if let Some(pe) = take_pairing_error(&mut pairing_rx).await {
                return Err(ConnectError::Pairing(pe));
            }
            return Err(e);
        }
    };
    match resp {
        ControlMsg::SessionReject { reason } => {
            // Busy is a definitive host verdict: report it directly. The host drops
            // the connection right after sending the reject, so the pairing stream's
            // read error ("application closed") would otherwise win the 500ms race in
            // take_pairing_error and mask the real reason.
            if !matches!(reason, removent_proto::RejectReason::Busy)
                && let Some(pe) = take_pairing_error(&mut pairing_rx).await
            {
                return Err(ConnectError::Pairing(pe));
            }
            return Err(ConnectError::Rejected(format!("{reason:?}")));
        }
        // Quick resume: only when the host accepted the token — it then skips
        // negotiation and we reuse the previous parameters. A rejected token
        // falls through to full negotiation so both ends stay in sync.
        ControlMsg::SessionAccept if resume_accepted && prev_ack.is_some() => {
            return build_session(conn, sink, source, prev_ack.unwrap(), cfg);
        }
        ControlMsg::SessionAccept => {}
        _ => unreachable!(),
    }

    // The host completes its pairing ruling before SessionAccept; here we consume the
    // pairing result, attributing a definite failure (wrong PIN) to Pairing. When the
    // host trusts us, the pairing task was never adjudicated, so a brief wait with no
    // result is treated as "no pairing happened".
    if let Some(rx) = pairing_rx.take() {
        match tokio::time::timeout(Duration::from_millis(500), rx).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(pe))) => return Err(ConnectError::Pairing(pe)),
            _ => {}
        }
    }
    if let Some(t) = pairing_task.take() {
        t.abort();
    }

    let offer = expect_msg(&mut source, "NegotiateOffer", NEGOTIATE_TIMEOUT, |m| {
        matches!(m, ControlMsg::NegotiateOffer { .. })
    })
    .await?;
    let ControlMsg::NegotiateOffer { n } = offer else {
        unreachable!()
    };
    let n = *n;

    // The client accepts the host's proposal, trimmed to the declared capabilities:
    // without the audio cap nothing would consume the decoded PCM (§5.2).
    sink.send(ControlMsg::NegotiateReply {
        ack: Box::new(NegotiateAck {
            video: n.video,
            audio: removent_proto::AudioParams {
                enabled: cfg.caps.audio && n.audio.enabled,
                ..n.audio
            },
            resume_token: None,
        }),
    })
    .await?;

    let final_ack = expect_msg(
        &mut source,
        "final NegotiateReply",
        NEGOTIATE_TIMEOUT,
        |m| matches!(m, ControlMsg::NegotiateReply { .. }),
    )
    .await?;
    let ControlMsg::NegotiateReply { ack } = final_ack else {
        unreachable!()
    };
    let ack = *ack;

    build_session(conn, sink, source, ack, cfg)
}

/// Build the session: media loops + control pump + clipboard poller.
fn build_session(
    conn: RvpConnection,
    mut sink: ControlSink,
    mut source: ControlSource,
    ack: NegotiateAck,
    cfg: ClientConfig,
) -> Result<ClientSession, ConnectError> {
    use futures::SinkExt;
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<ControlMsg>(64);
    let (decoded_bgra_tx, decoded_bgra_rx) = mpsc::channel(4);
    let (decoded_pcm_tx, decoded_pcm_rx) = mpsc::channel(128);

    let resume_token = Arc::new(Mutex::new(ack.resume_token));
    let last_kf = Arc::new(Mutex::new(None));
    let media_tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>> =
        Arc::new(Mutex::new(Vec::new()));

    // Media receive: stream count follows the negotiation result (when audio.enabled is
    // off there is only a video stream).
    let dispatch = spawn_media_loops(
        conn.clone(),
        decoded_bgra_tx,
        decoded_pcm_tx,
        ack.audio.enabled,
        cmd_tx.clone(),
        last_kf.clone(),
        media_tasks.clone(),
    );

    // Session-level clipboard state: the pump and poller share the suppress singleton
    // to prevent echo loops.
    let clip_state = cfg
        .local_clip
        .as_ref()
        .map(|clip| Arc::new(ClipSyncState::new(clip.as_ref())));
    let local_clip = cfg.local_clip.clone();
    let last_quality = Arc::new(Mutex::new(None));

    let quality = last_quality.clone();
    let pump_clip_state = clip_state.clone();
    let pump_resume_token = resume_token.clone();
    let pump = tokio::spawn(async move {
        let local_suppress = std::sync::atomic::AtomicU64::new(0);
        loop {
            let sink = &mut sink;
            tokio::select! {
                item = source.next() => {
                    let Some(item) = item else { break };
                    let Ok(item) = item else { break };
                    match item {
                        ControlItem::Msg(m) => match *m {
                            ControlMsg::ClipboardSync { seq, data, .. } => {
                                if let Some(clip) = local_clip.as_ref() {
                                    let suppress: &std::sync::atomic::AtomicU64 = pump_clip_state
                                        .as_ref()
                                        .map(|s| &s.suppress_cc)
                                        .unwrap_or(&local_suppress);
                                    if let Err(e) = removent_core::apply_incoming_clip(
                                        clip.as_ref(),
                                        suppress,
                                        &data,
                                    ) {
                                        tracing::warn!(err=%e, "clipboard apply failed");
                                    }
                                }
                                // An Ack confirms receipt, not that this endpoint had a
                                // local pasteboard bridge. Always acknowledge the message
                                // so a sender cannot remain blocked when clipboard support
                                // is disabled or unavailable on this side.
                                if sink.send(ControlMsg::ClipboardAck { seq }).await.is_err() {
                                    break;
                                }
                            }
                            ControlMsg::Ping { ts_us } => {
                                if sink.send(ControlMsg::Pong { ts_us }).await.is_err() {
                                    break;
                                }
                            }
                            ControlMsg::QualityControl {
                                bitrate_kbps,
                                fps,
                                scale,
                                ..
                            } => {
                                tracing::info!(bitrate_kbps, "host quality control");
                                *quality.lock().unwrap() = Some((bitrate_kbps, fps, scale));
                            }
                            // resume rotation (§7.4): the host sends back an ack with the new token after resume.
                            ControlMsg::NegotiateReply { ack } => {
                                if let Some(t) = ack.resume_token {
                                    *pump_resume_token.lock().unwrap() = Some(t);
                                }
                            }
                            ControlMsg::SessionEnd { .. } => break,
                            _ => {}
                        },
                        ControlItem::Skipped => continue,
                    }
                }
                maybe_cmd = cmd_rx.recv() => {
                    match maybe_cmd {
                        Some(msg @ ControlMsg::SessionEnd { .. }) => {
                            // Outbound SessionEnd must actually reach the peer before we exit.
                            let _ = sink.send(msg).await;
                            break;
                        }
                        None => break,
                        Some(msg) => {
                            if sink.send(msg).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        }
    });

    // Local clipboard changes → send to the peer (shares suppress state with the pump).
    let poller = cfg.local_clip.clone().map(|clip| {
        removent_core::spawn_clipboard_poller(
            clip,
            clip_state.clone().expect("clip_state with clip"),
            cmd_tx.clone(),
            250,
        )
    });

    let mut tasks = vec![dispatch, pump];
    if let Some(p) = poller {
        tasks.push(p);
    }

    Ok(ClientSession {
        conn,
        cmd_tx,
        resume_token,
        negotiated: ack,
        decoded_bgra_rx,
        decoded_pcm_rx,
        last_quality,
        last_kf,
        tasks,
        media_tasks,
    })
}

fn hex_encode(bytes: [u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Real system version (read once via sw_vers and cached).
fn os_version() -> String {
    static V: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    V.get_or_init(|| {
        std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".into())
    })
    .clone()
}

/// Quick resume within the 30s window after a disconnect (protocol.md §7.3/§7.4).
/// `prev_ack` is the previous session's `negotiated` parameters; after a successful
/// resume, `session.resume_token` is updated with the host's rotated new token.
pub async fn quick_resume(
    ep: removent_net::quinn::Endpoint,
    addr: SocketAddr,
    identity: &DeviceIdentity,
    cfg: ClientConfig,
    token: [u8; 16],
    prev_ack: NegotiateAck,
) -> Result<ClientSession, ConnectError> {
    connect_session(ep, addr, identity, cfg, Some(token), Some(prev_ack), None).await
}

// ---------------- pairing initiation ----------------

/// Returns (task handle, result channel). Pairing errors are reported over the channel
/// and attributed to Pairing by the connect flow.
fn spawn_pairing_initiator(
    conn: RvpConnection,
    identity: &DeviceIdentity,
    fp_self_full: String,
    fp_peer_full: String,
    pin_rx: PinInput,
) -> (
    tokio::task::JoinHandle<()>,
    oneshot::Receiver<Result<(), String>>,
) {
    let identity = identity.clone();
    let (tx, rx) = oneshot::channel();
    let handle = tokio::spawn(async move {
        let result = run_pairing(conn, &identity, &fp_self_full, &fp_peer_full, pin_rx).await;
        match &result {
            Ok(()) => tracing::info!("pairing completed"),
            Err(e) => tracing::warn!(err=%e, "pairing failed"),
        }
        let _ = tx.send(result.map_err(|e| e.to_string()));
    });
    (handle, rx)
}

async fn run_pairing(
    conn: RvpConnection,
    identity: &DeviceIdentity,
    fp_self_full: &str,
    fp_peer_full: &str,
    pin_rx: PinInput,
) -> Result<(), ConnectError> {
    let (mut psink, mut psource) = conn.open_pairing().await?;

    let (begin_msg, nonce_c) = client_begin(fp_self_full);
    psink.send(begin_msg).await.map_err(ConnectError::Net)?;

    let challenge = tokio::time::timeout(Duration::from_secs(15), psource.next())
        .await
        .map_err(|_| ConnectError::Timeout("pairing challenge"))?
        .transpose()
        .map_err(ConnectError::Net)?
        .ok_or(ConnectError::Timeout("pairing challenge"))?;
    let PairingMsg::Challenge {
        ref nonce_h,
        ref msg_h,
    } = challenge
    else {
        return Err(ConnectError::Pairing("expected Challenge".into()));
    };

    // The PIN is valid for 300s (protocol.md §4.3); the input wait is aligned with it.
    let pin = tokio::time::timeout(Duration::from_secs(300), pin_rx)
        .await
        .map_err(|_| ConnectError::Timeout("pin input"))?
        .map_err(|_| ConnectError::Pairing("pin channel dropped".into()))?;

    let (verify_msg, shared) = client_verify(
        &nonce_c,
        &challenge,
        fp_self_full,
        fp_peer_full,
        &pin,
        identity,
    )
    .map_err(ConnectError::Net)?;
    psink.send(verify_msg).await.map_err(ConnectError::Net)?;
    let _ = msg_h;

    let confirm = tokio::time::timeout(Duration::from_secs(15), psource.next())
        .await
        .map_err(|_| ConnectError::Timeout("pairing confirm"))?
        .transpose()
        .map_err(ConnectError::Net)?
        .ok_or(ConnectError::Timeout("pairing confirm"))?;
    let peer_vk = peer_verifying_key(&conn)
        .ok_or_else(|| ConnectError::Pairing("peer certificate key extract failed".into()))?;
    client_confirm_check(
        &shared,
        &nonce_c,
        nonce_h,
        fp_self_full,
        fp_peer_full,
        &confirm,
        &peer_vk,
    )
    .map_err(ConnectError::Net)?;
    Ok(())
}

/// Extract the long-term public key from the peer's TLS certificate (self-signed
/// ed25519) for pairing signature verification.
fn peer_verifying_key(conn: &RvpConnection) -> Option<ed25519_dalek::VerifyingKey> {
    let chain = conn
        .inner()
        .peer_identity()?
        .downcast::<Vec<removent_net::quinn::rustls::pki_types::CertificateDer<'static>>>()
        .ok()?;
    let der = chain.first()?;
    // Ed25519 SubjectPublicKeyInfo fixed prefix, followed by the 32-byte public key.
    const SPKI_PREFIX: [u8; 12] = [
        0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
    ];
    let bytes: &[u8] = der.as_ref();
    let pos = bytes
        .windows(SPKI_PREFIX.len())
        .position(|w| w == SPKI_PREFIX)?;
    let key: [u8; 32] = bytes
        .get(pos + SPKI_PREFIX.len()..pos + SPKI_PREFIX.len() + 32)?
        .try_into()
        .ok()?;
    ed25519_dalek::VerifyingKey::from_bytes(&key).ok()
}
// ---------------- media receive ----------------

/// Accept media uni-streams and dispatch by the first-byte stream_type to the
/// video/audio loops.
/// Stream count follows the negotiation result: when audio.enabled=false only the
/// video stream is expected (§5.2).
/// Sub-loop handles are registered into `media_tasks` so the session can abort them on
/// close/drop.
#[allow(clippy::too_many_arguments)]
fn spawn_media_loops(
    conn: RvpConnection,
    bgra_tx: mpsc::Sender<DecodedFrame>,
    pcm_tx: mpsc::Sender<Vec<i16>>,
    audio_enabled: bool,
    cmd_tx: mpsc::Sender<ControlMsg>,
    last_kf: Arc<Mutex<Option<Instant>>>,
    media_tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(media_dispatch_loop(
        conn,
        bgra_tx,
        pcm_tx,
        audio_enabled,
        cmd_tx,
        last_kf,
        media_tasks,
    ))
}

#[allow(clippy::too_many_arguments)]
async fn media_dispatch_loop(
    conn: RvpConnection,
    bgra_tx: mpsc::Sender<DecodedFrame>,
    pcm_tx: mpsc::Sender<Vec<i16>>,
    audio_enabled: bool,
    cmd_tx: mpsc::Sender<ControlMsg>,
    last_kf: Arc<Mutex<Option<Instant>>>,
    media_tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
) {
    use removent_proto::{STREAM_TYPE_AUDIO, STREAM_TYPE_VIDEO};
    let expected = if audio_enabled { 2 } else { 1 };
    for _ in 0..expected {
        let Ok(mut stream) = conn.accept_media_stream().await else {
            return;
        };
        let mut first = [0u8; 1];
        if stream.read_exact(&mut first).await.is_err() {
            continue;
        }
        let handle = match first[0] {
            STREAM_TYPE_VIDEO => tokio::spawn(video_recv_loop(
                stream,
                bgra_tx.clone(),
                cmd_tx.clone(),
                last_kf.clone(),
            )),
            STREAM_TYPE_AUDIO => tokio::spawn(audio_recv_loop(stream, pcm_tx.clone())),
            other => {
                tracing::warn!(t = other, "unknown media stream type");
                continue;
            }
        };
        media_tasks.lock().unwrap().push(handle);
    }
}

/// Rebuild the decoder on demand (hot) from the frame header: on CONFIG_CHANGED or a
/// resolution/codec change, rebuild using the keyframe's inline parameter sets
/// (protocol.md §6.1).
fn rebuild_decoder(hdr: &removent_proto::VideoFrameHeader, payload: &[u8]) -> Option<VideoDecoder> {
    let hevc = matches!(hdr.codec, removent_proto::CodecId::Hevc);
    let ps = extract_param_sets(payload, hevc);
    match VideoDecoder::new(hdr.codec, hdr.width as usize, hdr.height as usize, &ps) {
        Ok(d) => Some(d),
        Err(e) => {
            tracing::error!(err=%e, "decoder init failed");
            None
        }
    }
}

/// Dropping a JoinHandle does not stop the task; wrap it in abort-on-drop to prevent leaks.
struct AbortOnDrop(tokio::task::JoinHandle<()>);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Sanity bound on one compressed video payload: `payload_len` is an unchecked u32 from
/// the wire, and an absurd value must not force a multi-GiB allocation (a real keyframe
/// is orders of magnitude smaller).
const MAX_VIDEO_PAYLOAD_BYTES: u32 = 64 * 1024 * 1024;

/// Reject an absurd payload length before allocating; the stream is desynced/corrupt at
/// that point, so the caller drops it.
fn payload_len_ok(len: u32) -> bool {
    if len > MAX_VIDEO_PAYLOAD_BYTES {
        tracing::error!(
            payload_len = len,
            "video payload exceeds sanity bound, dropping stream"
        );
        return false;
    }
    true
}

/// The live decoder plus its output dimensions (from the header it was built from).
struct ActiveDecoder {
    dec: VideoDecoder,
    width: u32,
    height: u32,
}

/// Poll interval for the decoder drain task.
const DECODE_DRAIN_INTERVAL: Duration = Duration::from_millis(2);

/// Forward decoded frames independently of packet arrival. VTDecompressionSessionDecodeFrame
/// is asynchronous: decoded frames land in the decoder's internal channel from a VideoToolbox
/// callback thread, so draining only when the next packet arrives would add a constant
/// one-frame lag and strand the last frame before a static screen. This task polls the
/// decoder at a short interval and forwards whatever is finished.
fn spawn_decode_drain(
    decoder: Arc<Mutex<Option<ActiveDecoder>>>,
    bgra_tx: mpsc::Sender<DecodedFrame>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(DECODE_DRAIN_INTERVAL);
        loop {
            ticker.tick().await;
            // Collect finished frames; never hold the lock across .await.
            let ready: Vec<DecodedFrame> = {
                let g = decoder.lock().unwrap();
                let mut out = Vec::new();
                if let Some(active) = g.as_ref() {
                    while let Some(f) = active.dec.try_recv_decoded() {
                        out.push(DecodedFrame {
                            data: f.data,
                            width: active.width,
                            height: active.height,
                            pts_us: f.pts_us,
                        });
                    }
                }
                out
            };
            for frame in ready {
                if bgra_tx.send(frame).await.is_err() {
                    return;
                }
            }
        }
    })
}

/// Enqueue a KeyframeRequest, rate-limited to one per KEYFRAME_MIN_INTERVAL. The limiter
/// state is shared with [`ClientSession::request_keyframe`] so every request path (app,
/// first-frame, config-change, decode failure) honours the same 500ms minimum interval
/// (protocol.md §7.1).
fn request_keyframe(cmd_tx: &mpsc::Sender<ControlMsg>, last_kf: &Mutex<Option<Instant>>) {
    {
        let mut last = last_kf.lock().unwrap();
        let now = Instant::now();
        if last.is_some_and(|t| now.duration_since(t) < KEYFRAME_MIN_INTERVAL) {
            return;
        }
        *last = Some(now);
    }
    let _ = cmd_tx.try_send(ControlMsg::KeyframeRequest);
}

async fn video_recv_loop(
    mut stream: removent_net::quinn::RecvStream,
    bgra_tx: mpsc::Sender<DecodedFrame>,
    cmd_tx: mpsc::Sender<ControlMsg>,
    last_kf: Arc<Mutex<Option<Instant>>>,
) {
    use removent_proto::video_flags;
    // The first byte was already consumed; the remaining header is 26 bytes.
    let mut rest = [0u8; 26];
    if stream.read_exact(&mut rest).await.is_err() {
        return;
    }
    let mut head_bytes = [0u8; 27];
    head_bytes[0] = removent_proto::STREAM_TYPE_VIDEO;
    head_bytes[1..].copy_from_slice(&rest);
    let Ok((hdr0, _)) = parse_video_header(&head_bytes) else {
        tracing::error!("first video header parse failed");
        return;
    };

    // Shared with the drain task (aborted when this loop exits); decoded frames are
    // forwarded from there, not inline here.
    let decoder: Arc<Mutex<Option<ActiveDecoder>>> = Arc::new(Mutex::new(None));
    let _drain = AbortOnDrop(spawn_decode_drain(decoder.clone(), bgra_tx.clone()));
    // Current decoder configuration (codec, w, h), used to detect when a hot rebuild
    // is needed.
    let mut cur_cfg: Option<(removent_proto::CodecId, u16, u16)> = None;

    // First frame (should be a keyframe): read its payload and initialize the decoder.
    if !payload_len_ok(hdr0.payload_len) {
        return;
    }
    let mut first_payload = vec![0u8; hdr0.payload_len as usize];
    if stream.read_exact(&mut first_payload).await.is_err() {
        return;
    }
    if hdr0.is_keyframe() {
        match rebuild_decoder(&hdr0, &first_payload) {
            Some(d) => {
                if let Err(e) = d.decode_annexb(&first_payload, hdr0.pts_us) {
                    // First-frame decode failure: nudge the host for a fresh keyframe
                    // (rate-limited, shared limiter with the other request paths)
                    // instead of only logging and waiting for the periodic IDR.
                    tracing::warn!(err=%e, "first frame decode failed, requesting keyframe");
                    request_keyframe(&cmd_tx, &last_kf);
                }
                *decoder.lock().unwrap() = Some(ActiveDecoder {
                    dec: d,
                    width: hdr0.width as u32,
                    height: hdr0.height as u32,
                });
                cur_cfg = Some((hdr0.codec, hdr0.width, hdr0.height));
            }
            None => {
                // First-frame init failed: request a new keyframe to retry instead of
                // exiting silently.
                tracing::error!("first frame decoder init failed, requesting keyframe");
                request_keyframe(&cmd_tx, &last_kf);
            }
        }
    } else {
        tracing::warn!("first video frame is not a keyframe, requesting keyframe");
        request_keyframe(&cmd_tx, &last_kf);
    }

    loop {
        // The renderer is gone: stop reading (the drain task exits on send failure).
        if bgra_tx.is_closed() {
            return;
        }
        // Subsequent frames: full 27-byte header.
        if stream.read_exact(&mut head_bytes).await.is_err() {
            break;
        }
        let Ok((hdr, _)) = parse_video_header(&head_bytes) else {
            tracing::warn!(head = ?head_bytes, "bad video header, dropping stream");
            break;
        };
        if !payload_len_ok(hdr.payload_len) {
            break;
        }
        let mut payload = vec![0u8; hdr.payload_len as usize];
        if stream.read_exact(&mut payload).await.is_err() {
            break;
        }

        // CONFIG_CHANGED or a resolution/codec change → hot-rebuild the decoder from
        // this keyframe.
        let stale = cur_cfg != Some((hdr.codec, hdr.width, hdr.height));
        let need_rebuild = decoder.lock().unwrap().is_none() || hdr.config_changed() || stale;
        if need_rebuild {
            if hdr.flags & video_flags::KEYFRAME == 0 {
                // Config changed but no parameter sets: request a keyframe and skip
                // this frame.
                request_keyframe(&cmd_tx, &last_kf);
                continue;
            }
            match rebuild_decoder(&hdr, &payload) {
                Some(d) => {
                    tracing::info!(
                        codec = ?hdr.codec, w = hdr.width, h = hdr.height,
                        "video decoder (re)initialised"
                    );
                    *decoder.lock().unwrap() = Some(ActiveDecoder {
                        dec: d,
                        width: hdr.width as u32,
                        height: hdr.height as u32,
                    });
                    cur_cfg = Some((hdr.codec, hdr.width, hdr.height));
                }
                None => {
                    request_keyframe(&cmd_tx, &last_kf);
                    continue;
                }
            }
        }
        let result = {
            let g = decoder.lock().unwrap();
            g.as_ref()
                .map(|a| a.dec.decode_annexb(&payload, hdr.pts_us))
        };
        let Some(result) = result else { continue };
        if let Err(e) = result {
            // Mid-stream decode failure: nudge the host for a keyframe (rate-limited)
            // so recovery does not wait for the 2s periodic IDR (or forever on a
            // static screen).
            tracing::warn!(err=%e, "video decode failed, requesting keyframe");
            request_keyframe(&cmd_tx, &last_kf);
        }
    }
}

async fn audio_recv_loop(
    mut stream: removent_net::quinn::RecvStream,
    pcm_tx: mpsc::Sender<Vec<i16>>,
) {
    // Reading and playback are separated: a dedicated playback task holds the decoder
    // and jitter buffer (emits packets on a 10ms cadence); the read loop only frames
    // packets into the buffer, so playback pacing is unaffected by blocking network reads.
    // Media runs over ordered reliable QUIC streams, so the buffer adds no reorder
    // delay — in-order packets pass straight through.
    let jb = Arc::new(Mutex::new(JitterBuffer::new(60)));
    let _playout = AbortOnDrop(tokio::spawn(audio_playout_loop(jb.clone(), pcm_tx)));

    // On the wire every packet is [14-byte header (incl. type byte)][payload] (same as
    // video frames); the first packet's type byte was already consumed by the media
    // dispatcher, so only the first packet needs it patched back in.
    let mut head = [0u8; 14];
    head[0] = removent_proto::STREAM_TYPE_AUDIO;
    let mut first = true;
    loop {
        let r = if first {
            first = false;
            stream.read_exact(&mut head[1..]).await
        } else {
            stream.read_exact(&mut head).await
        };
        if r.is_err() {
            break;
        }
        if head[0] != removent_proto::STREAM_TYPE_AUDIO {
            tracing::warn!("audio stream type byte mismatch, dropping stream");
            break;
        }
        let Ok((hdr, _)) = parse_audio_header(&head) else {
            tracing::warn!("bad audio header, dropping stream");
            break;
        };
        let mut payload = vec![0u8; hdr.payload_len as usize];
        if stream.read_exact(&mut payload).await.is_err() {
            break;
        }
        jb.lock().unwrap().push(AudioPacketIn {
            seq: hdr.seq,
            pts_us: hdr.pts_us,
            flags: hdr.flags,
            payload,
        });
    }
}

/// Audio playback loop: emit packets on a 10ms cadence as they arrive (the transport
/// is an ordered reliable QUIC stream, so no reorder buffering is needed); a missing
/// playhead packet (only reachable after flood-cap eviction) goes through Opus PLC
/// (conceal); DTX packets are skipped without feeding the decoder.
async fn audio_playout_loop(jb: Arc<Mutex<JitterBuffer>>, pcm_tx: mpsc::Sender<Vec<i16>>) {
    let Ok(mut decoder) = AudioDecoder::new() else {
        tracing::warn!("audio decoder init failed");
        return;
    };
    let mut ticker = tokio::time::interval(Duration::from_millis(10));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let outcome = jb.lock().unwrap().pop();
        match outcome {
            PopOutcome::Packet(p) => {
                if p.flags & removent_proto::audio_flags::DTX != 0 {
                    continue;
                }
                match decoder.decode_frame(&p.payload) {
                    Ok(pcm) => {
                        if pcm_tx.send(pcm).await.is_err() {
                            return;
                        }
                    }
                    Err(e) => tracing::warn!(err=%e, "opus decode"),
                }
            }
            PopOutcome::Plc(_) => match decoder.conceal() {
                Ok(pcm) => {
                    if pcm_tx.send(pcm).await.is_err() {
                        return;
                    }
                }
                Err(e) => tracing::warn!(err=%e, "opus plc"),
            },
            PopOutcome::Wait => {}
        }
    }
}
