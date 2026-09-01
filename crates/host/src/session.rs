//! Host-side session engine: admission, pairing, negotiation, and media send loops.
//!
//! Media sources are injected via tokio mpsc channels (either the SCK capture
//! implementation or a test synthesizer), so the engine does not depend on TCC
//! permissions and can be verified headless. Message sequencing: protocol.md §4–5.

use crate::admission::{AdmissionDecision, decide};
use crate::input_sink::InputSink;
use crate::sender::SendGate;
use futures::{SinkExt, StreamExt};
use rand::RngCore;
use removent_core::{
    AdmissionMode, ClipSyncState, DeviceIdentity, PeerRecord, PeersStore, TextClipboard,
    adapt::{AdaptationController, Sample},
};
use removent_media_codec::VideoEncoder;
use removent_net::{ControlItem, ControlSink, ControlSource, RvpConnection};
use removent_proto::{
    AudioPacketHeader, Caps, CodecId, ControlMsg, HandshakeServer, KeyKind, MouseKind, Negotiate,
    NegotiateAck, PROTO_VERSION, RESUME_WINDOW_SECS, ScrollPhase, VideoParams, build_audio_packet,
    build_video_frame,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Error)]
pub enum HostError {
    #[error("net: {0}")]
    Net(#[from] removent_net::NetError),
    #[error("codec: {0}")]
    Codec(#[from] removent_media_codec::VideoError),
    #[error("rejected: {0}")]
    Rejected(String),
    #[error("timeout waiting for {0}")]
    Timeout(&'static str),
}

/// User interaction callbacks (implemented by the UI or by test auto-responders).
pub type PromptFuture = std::pin::Pin<Box<dyn Future<Output = bool> + Send>>;

pub struct HostInteractions {
    /// Show the PIN to the machine owner (non-blocking).
    pub show_pairing_pin: Box<dyn FnOnce(String) + Send>,
    /// Admission prompt: asynchronously awaits the user's ruling.
    pub admission_prompt: Box<dyn FnOnce(String, String) -> PromptFuture + Send>,
}

impl HostInteractions {
    /// For tests: forwards the PIN over a channel and auto-allows admission.
    pub fn auto_allow_with_pin_tx(pin_tx: oneshot::Sender<String>) -> Self {
        Self {
            show_pairing_pin: Box::new(move |pin| {
                let _ = pin_tx.send(pin);
            }),
            admission_prompt: Box::new(|_, _| {
                Box::pin(async { true }) as std::pin::Pin<Box<dyn Future<Output = bool> + Send>>
            }),
        }
    }
}

/// Host-side configuration.
pub struct HostConfig {
    pub device_name: String,
    pub admission: AdmissionMode,
    pub video_bitrate_kbps: u32,
    pub video_fps: u8,
    /// Real machines pass RealInputSink; tests pass a recorder.
    pub input_sink: Option<Arc<dyn InputSink>>,
    /// Local clipboard bridge (NSPasteboard on real machines; an in-memory impl in tests).
    pub local_clip: Option<Arc<dyn TextClipboard>>,
}

/// Media sources (host-side capture implementation or test injection).
#[derive(Clone)]
pub struct HostMediaFeeds {
    /// BGRA frames (stride=width*4) + pts in microseconds.
    pub video_tx: mpsc::Sender<(Vec<u8>, i64)>,
    /// 10ms stereo PCM frames with source presentation timestamps.
    pub audio_tx: mpsc::Sender<removent_media_capture::AudioFrame>,
}

/// Session handle produced once negotiation completes.
type SessionChannels = (
    mpsc::Sender<ControlMsg>,
    mpsc::Receiver<ControlMsg>,
    Arc<std::sync::Mutex<AdaptationController>>,
    mpsc::Sender<()>,
    mpsc::Receiver<()>,
    mpsc::Sender<u32>,
    mpsc::Receiver<u32>,
);

fn build_session_channels(bitrate_kbps: u32, fps: u8) -> SessionChannels {
    let (cmd_tx, cmd_rx) = mpsc::channel::<ControlMsg>(64);
    let (kf_tx, kf_rx) = mpsc::channel::<()>(4);
    let (bitrate_tx, bitrate_rx) = mpsc::channel::<u32>(4);
    let controller = Arc::new(std::sync::Mutex::new(AdaptationController::new(
        bitrate_kbps,
        fps,
        removent_core::QualityPreset::Auto,
    )));
    (
        cmd_tx, cmd_rx, controller, kf_tx, kf_rx, bitrate_tx, bitrate_rx,
    )
}

/// Start the clipboard poller and return the session-level shared state (the pump side
/// writes suppress into the same singleton to prevent echo loops).
/// The polling task terminates when the session is cancelled.
fn spawn_clip_poller(
    clip: Arc<dyn TextClipboard>,
    cmd_tx: mpsc::Sender<ControlMsg>,
    cancel: &CancellationToken,
) -> Arc<ClipSyncState> {
    let state = Arc::new(ClipSyncState::new(clip.as_ref()));
    let handle = removent_core::spawn_clipboard_poller(clip, state.clone(), cmd_tx, 250);
    let cancel = cancel.clone();
    tokio::spawn(async move {
        cancel.cancelled().await;
        handle.abort();
    });
    state
}

pub struct EstablishedSession {
    pub peer_fp_hex: String,
    pub ack: NegotiateAck,
    /// The quick-resume token issued for this session (or after rotation).
    pub resume_token: [u8; 16],
    /// App layer → peer control-message egress (SessionEnd, custom, etc.).
    pub cmd_tx: mpsc::Sender<ControlMsg>,
    /// Adaptation controller (used inside the pump; tests may read its state).
    pub controller: Arc<std::sync::Mutex<AdaptationController>>,
    /// Keyframe-request injection endpoint (forwarded to the video loop).
    pub keyframe_req_tx: mpsc::Sender<()>,
    /// Encoder bitrate update channel (for adaptive decisions).
    pub bitrate_tx: mpsc::Sender<u32>,
    /// Capability set requested by the peer (the control pump trims input
    /// injection/clipboard writes accordingly).
    pub peer_caps: Caps,
    /// Session-level clipboard sync state (shared with the poller; None when there is
    /// no local clipboard).
    pub clip_state: Option<Arc<ClipSyncState>>,
    /// Session-level stop token: cancelled automatically when the control pump exits,
    /// bringing down the media loops and poller with it.
    pub cancel: CancellationToken,
}

// ---------------- resume token registry ----------------

/// Resume registry entry. The validity window is anchored to the last time the
/// peer was seen (`issued_at` is refreshed at session teardown, protocol.md §7.3),
/// and `prev_token` tolerates exactly one lost rotation reply (§7.4).
struct ResumeEntry {
    issued_at: u64,
    token: [u8; 16],
    prev_token: Option<[u8; 16]>,
    ack: NegotiateAck,
    caps: Caps,
}

/// fp → resume entry.
type ResumeMap = HashMap<String, ResumeEntry>;

fn resume_store() -> &'static std::sync::Mutex<ResumeMap> {
    static STORE: std::sync::OnceLock<std::sync::Mutex<ResumeMap>> = std::sync::OnceLock::new();
    STORE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// Evict zombie entries older than RESUME_WINDOW_SECS.
fn sweep_expired(store: &mut ResumeMap) {
    let now = now_unix();
    store.retain(|_, e| now.saturating_sub(e.issued_at) <= RESUME_WINDOW_SECS);
}

fn remember_resume(
    fp: &str,
    token: &[u8; 16],
    ack: &NegotiateAck,
    caps: Caps,
    prev_token: Option<[u8; 16]>,
) {
    let mut store = resume_store().lock().unwrap();
    sweep_expired(&mut store);
    store.insert(
        fp.to_string(),
        ResumeEntry {
            issued_at: now_unix(),
            token: *token,
            prev_token,
            ack: ack.clone(),
            caps,
        },
    );
}

/// Re-anchor the validity window when a session ends: the 30s budget runs from
/// the last time the peer was seen, not from when the token was issued (a session
/// longer than the window must still be resumable after it ends).
pub fn refresh_resume(fp: &str) {
    let mut store = resume_store().lock().unwrap();
    if let Some(e) = store.get_mut(fp) {
        e.issued_at = now_unix();
    }
}

/// Tokens are single-use (protocol.md §7.4): invalidated immediately after consumption.
fn invalidate_resume(fp: &str) {
    resume_store().lock().unwrap().remove(fp);
}

/// The bool in the return value marks a match against the previous generation
/// (tolerated once for a lost rotation reply) rather than the current token.
fn validate_resume_full(fp: &str, token: &[u8; 16]) -> Option<(NegotiateAck, Caps, bool)> {
    let mut store = resume_store().lock().unwrap();
    sweep_expired(&mut store);
    let e = store.get(fp)?;
    if now_unix().saturating_sub(e.issued_at) > RESUME_WINDOW_SECS {
        return None;
    }
    if &e.token == token {
        Some((e.ack.clone(), e.caps, false))
    } else if e.prev_token.as_ref() == Some(token) {
        Some((e.ack.clone(), e.caps, true))
    } else {
        None
    }
}

pub fn validate_resume(fp: &str, token: &[u8; 16]) -> Option<NegotiateAck> {
    validate_resume_full(fp, token).map(|(ack, _, _)| ack)
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------- pairing Begin rate limit ----------------

/// Minimum interval between two pairing Begins from the same peer: a client
/// reconnecting in a tight loop must not spam the host with PIN popups.
const PAIRING_BEGIN_MIN_INTERVAL: Duration = Duration::from_secs(5);

/// Per-peer-fingerprint rate limiter for pairing Begin. Process-wide: the
/// attack it defends against reconnects per attempt, so per-connection state
/// would never trigger.
struct PairingBeginLimiter {
    last_begin: HashMap<String, std::time::Instant>,
}

impl PairingBeginLimiter {
    /// Whether a Begin from `peer_fp` is allowed; records the attempt.
    fn allow(&mut self, peer_fp: &str, now: std::time::Instant) -> bool {
        // Bound the map: entries older than 10 minutes cannot block anything.
        if self.last_begin.len() >= 256 {
            self.last_begin
                .retain(|_, t| now.duration_since(*t) <= Duration::from_secs(600));
        }
        match self.last_begin.get(peer_fp) {
            Some(t) if now.duration_since(*t) < PAIRING_BEGIN_MIN_INTERVAL => false,
            _ => {
                self.last_begin.insert(peer_fp.to_string(), now);
                true
            }
        }
    }
}

fn pairing_begin_limiter() -> &'static std::sync::Mutex<PairingBeginLimiter> {
    static LIMITER: std::sync::OnceLock<std::sync::Mutex<PairingBeginLimiter>> =
        std::sync::OnceLock::new();
    LIMITER.get_or_init(|| {
        std::sync::Mutex::new(PairingBeginLimiter {
            last_begin: HashMap::new(),
        })
    })
}

// ---------------- main flow ----------------

async fn expect_msg(
    source: &mut ControlSource,
    what: &'static str,
    pred: impl Fn(&ControlMsg) -> bool,
) -> Result<ControlMsg, HostError> {
    loop {
        let item = tokio::time::timeout(Duration::from_secs(10), source.next())
            .await
            .map_err(|_| HostError::Timeout(what))?
            .transpose()
            .map_err(HostError::Net)?
            .ok_or(HostError::Timeout(what))?;
        match item {
            ControlItem::Msg(m) => {
                if pred(&m) {
                    return Ok(*m);
                }
            }
            ControlItem::Skipped => continue,
        }
    }
}

/// Serve one inbound connection through negotiation and return the session handle;
/// the caller then spawns [`spawn_video_loop`] / [`spawn_audio_loop`] / the control-pump task.
pub async fn serve_connection(
    conn: RvpConnection,
    identity: &DeviceIdentity,
    peers: &mut PeersStore,
    cfg: &HostConfig,
    interactions: HostInteractions,
    main_display: removent_proto::DisplayInfo,
) -> Result<
    (
        EstablishedSession,
        mpsc::Receiver<()>,
        mpsc::Receiver<u32>,
        mpsc::Receiver<ControlMsg>,
        ControlSink,
        ControlSource,
    ),
    HostError,
> {
    let peer_fp = conn
        .peer_fingerprint()
        .map(hex::encode)
        .ok_or_else(|| HostError::Rejected("peer fingerprint missing".into()))?;

    // The resume token is validated exactly once, inside the handshake closure, and
    // the verdict is reused for the branch below: two independent validations could
    // straddle the 30s window boundary and desynchronise the two ends (the host
    // taking the resume path while the client was told resume_accepted=false, or
    // vice versa).
    let resume_verdict: std::sync::Mutex<Option<(NegotiateAck, Caps, bool)>> =
        std::sync::Mutex::new(None);
    let (hello, mut sink, mut source) = conn
        .accept_handshake(|hello| {
            let verdict = hello
                .hello
                .resume_token
                .as_ref()
                .and_then(|t| validate_resume_full(&peer_fp, t));
            let accepted = verdict.is_some();
            *resume_verdict.lock().unwrap() = verdict;
            HandshakeServer {
                proto_version: PROTO_VERSION,
                // Honest declaration: file-transfer/two-way-audio/hdr are all unimplemented.
                feature_bits: 0,
                device_name: cfg.device_name.clone(),
                // Honest resume verdict (§7.4): Some(true) only when the presented token
                // validated; Some(false) tells the client to fall back to full
                // negotiation instead of desynchronising the two ends.
                resume_accepted: hello.hello.resume_token.as_ref().map(|_| accepted),
                // A trusted client skips pairing (and its PIN prompt) entirely.
                peer_known: peers.by_fingerprint(&peer_fp).is_some(),
            }
        })
        .await?;

    // Quick-resume path: skips pairing/admission and reuses the last negotiation
    // parameters; the old token is single-use and rotated after consumption (§7.4).
    if let Some(token) = hello.hello.resume_token
        && let Some((prev_ack, peer_caps, matched_prev)) = resume_verdict.into_inner().unwrap()
    {
        invalidate_resume(&peer_fp);
        let new_token = new_token();
        let mut ack = prev_ack;
        ack.resume_token = Some(new_token);
        // Tolerate one lost rotation reply: the just-consumed current token stays
        // valid once more as the previous generation, so a client that never
        // received the new token can still resume; a consumed previous-generation
        // token is retired for good.
        let prev_token = if matched_prev { None } else { Some(token) };
        remember_resume(&peer_fp, &new_token, &ack, peer_caps, prev_token);
        sink.send(ControlMsg::SessionAccept).await?;
        // Deliver the rotated new token to the client (the resume path has no full negotiation).
        sink.send(ControlMsg::NegotiateReply {
            ack: Box::new(ack.clone()),
        })
        .await?;
        let cancel = CancellationToken::new();
        let (cmd_tx, cmd_rx, controller, kf_tx, kf_rx, bitrate_tx, bitrate_rx) =
            build_session_channels(cfg.video_bitrate_kbps, cfg.video_fps);
        let clip_state = cfg
            .local_clip
            .clone()
            .map(|clip| spawn_clip_poller(clip, cmd_tx.clone(), &cancel));
        return Ok((
            EstablishedSession {
                peer_fp_hex: peer_fp,
                ack,
                resume_token: new_token,
                cmd_tx: cmd_tx.clone(),
                controller,
                keyframe_req_tx: kf_tx.clone(),
                bitrate_tx: bitrate_tx.clone(),
                peer_caps,
                clip_state,
                cancel,
            },
            kf_rx,
            bitrate_rx,
            cmd_rx,
            sink,
            source,
        ));
    }

    // First connection: an unknown peer completes pairing inline first (the client opens
    // the pairing stream right after the handshake), and only then do we handle
    // SessionRequest (protocol.md §4.3 → §4.4 ordering).
    if peers.by_fingerprint(&peer_fp).is_none() {
        let peer_name = hello.hello.device_name.clone();
        run_pairing(
            &conn,
            identity,
            peers,
            &peer_fp,
            peer_name,
            interactions.show_pairing_pin,
        )
        .await?;
    }

    // First connection: wait for SessionRequest.
    let req = expect_msg(&mut source, "SessionRequest", |m| {
        matches!(m, ControlMsg::SessionRequest { .. })
    })
    .await?;
    let ControlMsg::SessionRequest { caps } = req else {
        unreachable!()
    };

    // Admission ruling.
    match decide(cfg.admission, peers, &peer_fp, caps) {
        AdmissionDecision::Allow => {}
        AdmissionDecision::Ask => {
            let short = peer_fp.chars().take(16).collect::<String>();
            // protocol.md §4.4: an admission prompt with no response within 30s is treated as a rejection.
            let prompt = (interactions.admission_prompt)(hello.hello.device_name.clone(), short);
            let allowed = tokio::time::timeout(Duration::from_secs(30), prompt)
                .await
                .unwrap_or(false);
            if !allowed {
                sink.send(ControlMsg::SessionReject {
                    reason: removent_proto::RejectReason::Denied,
                })
                .await?;
                return Err(HostError::Rejected("user denied or prompt timeout".into()));
            }
            // "This time only": peers records the identity only, without a long-term
            // grant (protocol.md §4.3); an existing record (trusted or
            // grant-expansion flow) is not overwritten.
            if peers.by_fingerprint(&peer_fp).is_none() {
                let _ = peers.upsert(PeerRecord {
                    fingerprint: peer_fp.clone(),
                    name: hello.hello.device_name.clone(),
                    short_fp: peer_fp.chars().take(16).collect(),
                    granted_caps: Caps::none(),
                    trusted: false,
                    added_at_unix: now_unix(),
                    last_connected_unix: now_unix(),
                });
            }
        }
        AdmissionDecision::Deny(reason) => {
            sink.send(ControlMsg::SessionReject {
                reason: removent_proto::RejectReason::Denied,
            })
            .await?;
            return Err(HostError::Rejected(reason.into()));
        }
    }

    // Accept and negotiate.
    sink.send(ControlMsg::SessionAccept).await?;
    let displays = vec![main_display];
    let selected = displays[0].id;
    let codec = CodecId::Hevc; // the client may downgrade in NegotiateReply
    let negotiate = Negotiate {
        displays,
        selected_display: selected,
        video: VideoParams {
            codec,
            max_fps: cfg.video_fps,
            max_bitrate_kbps: cfg.video_bitrate_kbps,
            initial_scale: 1.0,
        },
        audio: removent_proto::AudioParams::default(),
    };
    sink.send(ControlMsg::NegotiateOffer {
        n: Box::new(negotiate.clone()),
    })
    .await?;

    let reply = expect_msg(&mut source, "NegotiateReply", |m| {
        matches!(m, ControlMsg::NegotiateReply { .. })
    })
    .await?;
    let ControlMsg::NegotiateReply { ack } = reply else {
        unreachable!()
    };

    // Issue a resume token and send it back.
    let mut ack = *ack;
    let token = new_token();
    remember_resume(&peer_fp, &token, &ack, caps, None);
    ack.resume_token = Some(token);
    sink.send(ControlMsg::NegotiateReply {
        ack: Box::new(ack.clone()),
    })
    .await?;

    let cancel = CancellationToken::new();
    let (cmd_tx, cmd_rx, controller, kf_tx, kf_rx, bitrate_tx, bitrate_rx) =
        build_session_channels(cfg.video_bitrate_kbps, cfg.video_fps);
    let clip_state = cfg
        .local_clip
        .clone()
        .map(|clip| spawn_clip_poller(clip, cmd_tx.clone(), &cancel));
    Ok((
        EstablishedSession {
            peer_fp_hex: peer_fp,
            ack,
            resume_token: token,
            cmd_tx,
            controller,
            keyframe_req_tx: kf_tx,
            bitrate_tx,
            peer_caps: caps,
            clip_state,
            cancel,
        },
        kf_rx,
        bitrate_rx,
        cmd_rx,
        sink,
        source,
    ))
}

async fn run_pairing(
    conn: &RvpConnection,
    identity: &DeviceIdentity,
    peers: &mut PeersStore,
    peer_fp: &str,
    peer_name: String,
    show_pin: Box<dyn FnOnce(String) + Send>,
) -> Result<(), HostError> {
    // The pairing stream is opened by the admitting side (host): the client accepts
    // while waiting. Bound the wait for the peer to open the stream: without a
    // deadline an unknown peer that stalls after the handshake would hold the
    // single session slot forever (the QUIC keepalive defeats the idle timeout).
    let (mut psink, mut psource) =
        tokio::time::timeout(Duration::from_secs(15), conn.accept_pairing())
            .await
            .map_err(|_| HostError::Timeout("pairing stream"))??;
    // Bound the Begin read the same way (15s stream-arrival budget); the PIN entry
    // window itself stays 300s (protocol.md §4.3).
    let begin = tokio::time::timeout(Duration::from_secs(15), psource.next())
        .await
        .map_err(|_| HostError::Timeout("pairing begin"))?
        .transpose()
        .map_err(HostError::Net)?
        .ok_or(HostError::Timeout("pairing begin"))?;
    // Rate limit per peer BEFORE generating/showing the PIN: an over-limit
    // Begin is rejected without producing a popup.
    if !pairing_begin_limiter()
        .lock()
        .unwrap()
        .allow(peer_fp, std::time::Instant::now())
    {
        tracing::warn!("pairing Begin rate limited, PIN not shown");
        return Err(HostError::Rejected("pairing begin too frequent".into()));
    }
    let hs = removent_net::host_on_begin(&begin)?;
    show_pin(hs.pin.clone());
    psink.send(hs.reply.clone()).await.map_err(HostError::Net)?;

    let verify = tokio::time::timeout(Duration::from_secs(300), psource.next())
        .await
        .map_err(|_| HostError::Timeout("pairing verify"))?
        .transpose()
        .map_err(HostError::Net)?
        .ok_or(HostError::Timeout("pairing verify"))?;
    let (confirm, shared) = removent_net::host_verify(hs, &verify, identity, peer_fp)?;
    // Deliver the Confirm (including ok=false) to the client before terminating, so the
    // client can attribute the failure to a wrong PIN.
    psink.send(confirm).await.map_err(HostError::Net)?;
    if shared.is_none() {
        return Err(HostError::Rejected("pin mismatch".into()));
    }

    peers
        .upsert(PeerRecord {
            fingerprint: peer_fp.to_string(),
            name: peer_name,
            short_fp: peer_fp.chars().take(16).collect(),
            granted_caps: Caps::all(),
            trusted: true,
            added_at_unix: now_unix(),
            last_connected_unix: now_unix(),
        })
        .map_err(|e| HostError::Rejected(e.to_string()))?;
    Ok(())
}

fn new_token() -> [u8; 16] {
    let mut t = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut t);
    t
}

// ---------------- media loops ----------------

/// Video send loop: encode → backpressure gate → write to the media uni-stream.
/// Returns the JoinHandle; the loop exits immediately when `cancel` fires
/// (control-pump exit / session end). `fatal_tx` receives a SessionEnd when the
/// encoder fails fatally (init at session start or rebuild after a resolution
/// change), so the client gets an explicit end instead of a silent video hang.
/// `capture_display_id` identifies the captured display for resolution-change
/// rebuilds; `downgrade_tx` forwards SendGate backpressure downgrades to the
/// control pump (which routes them through the adaptation controller).
/// `input` is the session's input sink: encoder rebuilds update its capture
/// dims so it does not keep rescaling coordinates against the stale size.
#[allow(clippy::too_many_arguments)]
pub fn spawn_video_loop(
    mut stream: removent_net::quinn::SendStream,
    mut video_rx: mpsc::Receiver<(Vec<u8>, i64)>,
    mut keyframe_req_rx: mpsc::Receiver<()>,
    mut bitrate_rx: mpsc::Receiver<u32>,
    codec: CodecId,
    width: usize,
    height: usize,
    bitrate_kbps: u32,
    fps: u8,
    cancel: CancellationToken,
    fatal_tx: Option<mpsc::Sender<ControlMsg>>,
    capture_display_id: u64,
    downgrade_tx: Option<mpsc::Sender<()>>,
    input: Option<Arc<dyn InputSink>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut cur_bitrate_kbps = bitrate_kbps;
        let mut encoder = match VideoEncoder::new(codec, width, height, cur_bitrate_kbps, fps) {
            Ok(e) => e,
            Err(e) => {
                tracing::error!(err=%e, "encoder init failed");
                if let Some(tx) = &fatal_tx {
                    let _ = tx
                        .send(ControlMsg::SessionEnd {
                            reason: removent_proto::EndReason::InternalError,
                        })
                        .await;
                }
                return;
            }
        };
        let mut gate = SendGate::new();
        let mut width = width;
        let mut height = height;
        // Set after an encoder rebuild: the next keyframe carries CONFIG_CHANGED
        // so the client hot-rebuilds its decoder (protocol.md §6.1).
        let mut config_changed_pending = false;
        let frame_id = std::cell::Cell::new(0u64);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                Some(kbps) = bitrate_rx.recv() => {
                    cur_bitrate_kbps = kbps;
                    if let Err(e) = encoder.set_bitrate_kbps(kbps) {
                        tracing::warn!(err=%e, "set bitrate failed");
                    }
                }
                Some(()) = keyframe_req_rx.recv() => encoder.request_keyframe(),
                item = video_rx.recv() => {
                    let Some((bgra, pts_us)) = item else { break };
                    let frames = match encoder.encode_bgra(&bgra, pts_us) {
                        Ok(f) => f,
                        Err(removent_media_codec::VideoError::PixelSizeMismatch { got, .. }) => {
                            // Capture resolution changed mid-stream: rebuild the
                            // encoder at the new dimensions, force an IDR and flag
                            // the next keyframe CONFIG_CHANGED so the client
                            // decoder hot-rebuilds.
                            match current_capture_dims(capture_display_id) {
                                Some((nw, nh)) if nw * nh * 4 == got => {
                                    match VideoEncoder::new(codec, nw, nh, cur_bitrate_kbps, fps) {
                                        Ok(mut e) => {
                                            e.request_keyframe();
                                            encoder = e;
                                            width = nw;
                                            height = nh;
                                            config_changed_pending = true;
                                            // The peer's coordinates now refer to
                                            // the new frame size; keep the input
                                            // sink's rescale base in sync.
                                            if let Some(sink) = &input {
                                                sink.set_capture_dims(nw as u32, nh as u32);
                                            }
                                            tracing::info!(w = nw, h = nh, "encoder rebuilt after resolution change");
                                        }
                                        Err(e) => {
                                            tracing::error!(err=%e, "encoder rebuild failed");
                                            if let Some(tx) = &fatal_tx {
                                                let _ = tx
                                                    .send(ControlMsg::SessionEnd {
                                                        reason: removent_proto::EndReason::InternalError,
                                                    })
                                                    .await;
                                            }
                                            return;
                                        }
                                    }
                                }
                                _ => tracing::warn!(got, "frame size mismatch, no matching display; dropping frame"),
                            }
                            continue;
                        }
                        Err(e) => { tracing::warn!(err=%e, "video encode failed"); continue; }
                    };
                    for ef in frames {
                        match gate.on_submit(ef.keyframe) {
                            crate::sender::SendAction::DropFrame => continue,
                            act => {
                                if act == crate::sender::SendAction::SendAndDowngrade {
                                    encoder.request_keyframe();
                                }
                                // Backpressure downgrade (protocol.md §6.4):
                                // signal the control pump instead of writing
                                // the encoder here — the pump routes it through
                                // the adaptation controller, the single owner of
                                // bitrate state. (Writing here directly made the
                                // gate's ×0.7 and the controller's ×1.25 fight.)
                                if gate.take_downgrade_request()
                                    && let Some(tx) = &downgrade_tx
                                {
                                    let _ = tx.try_send(());
                                }
                                frame_id.set(frame_id.get() + 1);
                                let mut flags = if ef.keyframe { removent_proto::video_flags::KEYFRAME } else { 0 };
                                if ef.keyframe && config_changed_pending {
                                    flags |= removent_proto::video_flags::CONFIG_CHANGED;
                                    config_changed_pending = false;
                                }
                                let hdr = removent_proto::VideoFrameHeader {
                                    frame_id: frame_id.get(),
                                    pts_us: ef.pts_us,
                                    flags,
                                    codec,
                                    width: width as u16,
                                    height: height as u16,
                                    payload_len: annexb_len_of(&ef),
                                };
                                let wire = build_video_frame(&hdr, &ef.data);
                                // on_sent only after the write completes (success or
                                // failure), so `queued` reflects the real backlog and
                                // the soft/hard limits can actually engage.
                                let write_result = stream.write_all(&wire).await;
                                gate.on_sent();
                                if write_result.is_err() {
                                    return;
                                }
                            }
                        }
                    }
                }
            }
        }
    })
}

/// Capture bounding box: the encoder input never exceeds 1920×1080.
pub const MAX_CAPTURE_W: u32 = 1920;
pub const MAX_CAPTURE_H: u32 = 1080;

/// Fits display pixel dims into the capture bounding box, preserving the
/// display's aspect ratio (an independent per-axis clamp would stretch e.g.
/// 16:10 panels). Never upscales; results are forced even for the encoder.
pub fn fit_capture_dims(w_px: u32, h_px: u32) -> (u32, u32) {
    let s = (MAX_CAPTURE_W as f64 / w_px as f64)
        .min(MAX_CAPTURE_H as f64 / h_px as f64)
        .min(1.0);
    let w = ((w_px as f64 * s) as u32) & !1;
    let h = ((h_px as f64 * s) as u32) & !1;
    (w.max(2), h.max(2))
}

/// Current capture dimensions for `display_id`, mirroring the runner's sizing
/// rule (the captured display's pixels aspect-fit into 1920×1080). Falls back
/// to the first display when the id is unknown; None in headless environments.
fn current_capture_dims(display_id: u64) -> Option<(usize, usize)> {
    let list = removent_input::display_list();
    let d = list.iter().find(|d| d.id == display_id).or(list.first())?;
    let (w, h) = fit_capture_dims(d.w_px, d.h_px);
    Some((w as usize, h as usize))
}

fn annexb_len_of(ef: &removent_media_codec::EncodedVideoFrame) -> u32 {
    ef.data.len() as u32
}

/// Audio send loop: Opus encode → audio packet stream. Exits when `cancel` fires.
pub fn spawn_audio_loop(
    mut stream: removent_net::quinn::SendStream,
    mut audio_rx: mpsc::Receiver<removent_media_capture::AudioFrame>,
    bitrate_kbps: u32,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut encoder = match removent_media_codec::AudioEncoder::new(
            bitrate_kbps,
            removent_media_codec::Application::Audio,
        ) {
            Ok(e) => e,
            Err(e) => {
                tracing::error!(err=%e, "audio encoder init failed");
                return;
            }
        };
        loop {
            let frame = tokio::select! {
                _ = cancel.cancelled() => break,
                item = audio_rx.recv() => {
                    let Some(frame) = item else { break };
                    frame
                }
            };
            let (seq, packet) = match encoder.encode_frame(&frame.samples) {
                Ok(x) => x,
                Err(e) => {
                    tracing::warn!(err=%e, "opus encode failed");
                    continue;
                }
            };
            let hdr = AudioPacketHeader {
                seq,
                // Source presentation timestamp (mach timebase, same epoch as
                // the video pts) so the client can lip-sync A/V.
                pts_us: frame.pts_micros,
                flags: 0,
                payload_len: packet.len() as u16,
            };
            let wire = build_audio_packet(&hdr, &packet);
            if stream.write_all(&wire).await.is_err() {
                return;
            }
        }
    })
}

/// Resident processing pump for the control stream: Pong replies, KeyframeRequest
/// forwarding (rate-limited), StatsReport → adaptation controller → QualityControl delivery.
pub struct ControlPumpDeps {
    /// Keyframe injection endpoint for the video loop (None when there is no video loop).
    pub kf_tx: Option<mpsc::Sender<()>>,
    pub controller: Option<std::sync::Arc<std::sync::Mutex<AdaptationController>>>,
    pub window_ms: u64,
    pub input: Option<Arc<dyn InputSink>>,
    pub local_clip: Option<Arc<dyn TextClipboard>>,
    pub bitrate_tx: Option<mpsc::Sender<u32>>,
    /// Peer's negotiated capability set: without input/clipboard, the corresponding
    /// messages are ignored.
    pub caps: Caps,
    /// Session-level clipboard state (shares the suppress singleton with the poller to
    /// prevent echo loops).
    pub clip_state: Option<Arc<ClipSyncState>>,
    /// Session-level stop token: cancelled when the pump exits, bringing down the media loops.
    pub cancel: CancellationToken,
    /// Peer fingerprint hex: on pump exit (= session end) the resume token's
    /// validity window is re-anchored to this moment (protocol.md §7.3).
    pub peer_fp: Option<String>,
    /// SendGate backpressure downgrade signals from the video loop (routed
    /// through the adaptation controller so it stays the single bitrate owner).
    pub downgrade_rx: Option<mpsc::Receiver<()>>,
}

/// Minimum interval between KeyframeRequests (protocol.md §7.1).
const KEYFRAME_MIN_INTERVAL: Duration = Duration::from_millis(500);

/// Per-session input injection rate limit (events/second, burst = 1s worth).
const INPUT_RATE_LIMIT_PER_SEC: f64 = 600.0;

/// Simple token bucket for rate limiting high-frequency input events.
pub struct TokenBucket {
    tokens: f64,
    capacity: f64,
    refill_per_sec: f64,
    last: std::time::Instant,
}

impl TokenBucket {
    pub fn new(rate_per_sec: f64) -> Self {
        Self {
            tokens: rate_per_sec,
            capacity: rate_per_sec,
            refill_per_sec: rate_per_sec,
            last: std::time::Instant::now(),
        }
    }

    pub fn try_take(&mut self) -> bool {
        self.try_take_at(std::time::Instant::now())
    }

    fn try_take_at(&mut self, now: std::time::Instant) -> bool {
        let elapsed = now.duration_since(self.last).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        self.last = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Control pump: inbound handling (Pong/keyframe/stats adaptation/input
/// injection/clipboard application) plus outbound command forwarding. The only task
/// holding the sink for the duration of the session.
pub fn spawn_control_pump(
    mut source: ControlSource,
    mut sink: ControlSink,
    mut deps: ControlPumpDeps,
    mut cmd_rx: mpsc::Receiver<ControlMsg>,
) -> tokio::task::JoinHandle<()> {
    use futures::SinkExt;
    tokio::spawn(async move {
        let cancel = deps.cancel.clone();
        let mut last_kf: Option<std::time::Instant> = None;
        let mut input_bucket = TokenBucket::new(INPUT_RATE_LIMIT_PER_SEC);
        let mut release_tracker = crate::input_sink::InputReleaseTracker::default();
        let mut downgrade_rx = deps.downgrade_rx.take();
        // Injection failures (e.g. Accessibility permission revoked mid-session)
        // are logged at most once per 5s to avoid flooding at 600 events/s.
        let mut last_inject_warn: Option<std::time::Instant> = None;
        let mut note_inject_err = |what: &str, e: String| {
            if last_inject_warn.is_none_or(|t| t.elapsed() >= Duration::from_secs(5)) {
                tracing::warn!(%what, err=%e, "input injection failed (accessibility permission revoked?)");
                last_inject_warn = Some(std::time::Instant::now());
            }
        };
        // Throttle for forwarding SendGate downgrade signals (the gate
        // re-signals per frame while the backlog persists).
        let mut last_gate_fwd: Option<std::time::Instant> = None;
        // Fallback suppress when there is no shared clip_state (does not prevent echo
        // loops, only makes writes possible).
        let local_suppress = std::sync::atomic::AtomicU64::new(0);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                item = source.next() => {
                    let Some(item) = item else { break };
                    let Ok(item) = item else { break };
                    match item {
                        ControlItem::Msg(m) => match *m {
                            ControlMsg::Ping { ts_us } => {
                                if sink.send(ControlMsg::Pong { ts_us }).await.is_err() {
                                    break;
                                }
                            }
                            ControlMsg::KeyframeRequest => {
                                // Rate limit: minimum interval 500ms (protocol.md §7.1).
                                let now = std::time::Instant::now();
                                if last_kf.is_none_or(|t| now.duration_since(t) >= KEYFRAME_MIN_INTERVAL) {
                                    last_kf = Some(now);
                                    if let Some(kf) = deps.kf_tx.as_ref() {
                                        let _ = kf.try_send(());
                                    }
                                }
                            }
                            ControlMsg::StatsReport {
                                rtt_ms,
                                loss_pct,
                                recv_kbps,
                                jitter_ms,
                                ..
                            } => {
                                let Some(controller) = deps.controller.as_ref() else {
                                    continue;
                                };
                                let decision = controller.lock().unwrap().on_sample(
                                    &Sample {
                                        rtt_ms,
                                        loss_pct,
                                        recv_kbps,
                                        jitter_ms,
                                    },
                                    deps.window_ms,
                                );
                                if let Some(st) = decision {
                                    tracing::info!(?st, "adaptation decision");
                                    let qc = ControlMsg::QualityControl {
                                        bitrate_kbps: st.bitrate_kbps,
                                        fps: st.fps,
                                        scale: st.scale,
                                    };
                                    if sink.send(qc).await.is_err() {
                                        break;
                                    }
                                    if let Some(kf) = deps.kf_tx.as_ref() {
                                        let _ = kf.try_send(());
                                    }
                                    if let Some(bt) = deps.bitrate_tx.as_ref() {
                                        let _ = bt.try_send(st.bitrate_kbps);
                                    }
                                }
                            }
                            ControlMsg::MouseEvent { display_id, x_px, y_px, buttons, kind } => {
                                if !deps.caps.input {
                                    tracing::warn!("mouse event from client without input cap, ignored");
                                    continue;
                                }
                                // Release events bypass the rate limiter: dropping
                                // a button-up would leave the button stuck on the
                                // host until session teardown.
                                let bypass = matches!(
                                    kind,
                                    MouseKind::LeftUp | MouseKind::RightUp | MouseKind::MiddleUp
                                );
                                if !bypass && !input_bucket.try_take() {
                                    tracing::warn!("input rate limit exceeded, dropping mouse event");
                                    continue;
                                }
                                release_tracker.note_mouse(display_id, x_px, y_px, kind);
                                if let Some(input) = deps.input.as_ref()
                                    && let Err(e) = input.mouse(display_id, x_px, y_px, buttons, kind)
                                {
                                    note_inject_err("mouse", e);
                                }
                            }
                            ControlMsg::KeyEvent { vk_code, modifiers, kind, unicode } => {
                                if !deps.caps.input {
                                    tracing::warn!("key event from client without input cap, ignored");
                                    continue;
                                }
                                // Key releases and modifier state changes bypass
                                // the rate limiter (stuck keys otherwise).
                                let bypass = matches!(kind, KeyKind::Up | KeyKind::FlagsChanged);
                                if !bypass && !input_bucket.try_take() {
                                    tracing::warn!("input rate limit exceeded, dropping key event");
                                    continue;
                                }
                                release_tracker.note_key(vk_code, modifiers, kind);
                                if let Some(input) = deps.input.as_ref()
                                    && let Err(e) = input.key(vk_code, modifiers, kind, unicode)
                                {
                                    note_inject_err("key", e);
                                }
                            }
                            ControlMsg::ScrollEvent { display_id, dx_mm, dy_mm, phase } => {
                                if !deps.caps.input {
                                    tracing::warn!("scroll event from client without input cap, ignored");
                                    continue;
                                }
                                // Scroll-end phases bypass the rate limiter so a
                                // gesture never lingers in "scrolling" state.
                                if phase != ScrollPhase::Ended && !input_bucket.try_take() {
                                    tracing::warn!("input rate limit exceeded, dropping scroll event");
                                    continue;
                                }
                                if let Some(input) = deps.input.as_ref()
                                    && let Err(e) = input.scroll(display_id, dx_mm, dy_mm, phase)
                                {
                                    note_inject_err("scroll", e);
                                }
                            }
                            ControlMsg::ClipboardSync { seq, data, .. } => {
                                if !deps.caps.clipboard {
                                    tracing::warn!("clipboard sync from client without clipboard cap, ignored");
                                    continue;
                                }
                                if let Some(clip) = deps.local_clip.as_ref() {
                                    // Shares the suppress singleton with the poller: after writing remote
                                    // content, the poller skips echoing it back.
                                    let suppress: &std::sync::atomic::AtomicU64 = deps
                                        .clip_state
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
                                    if sink.send(ControlMsg::ClipboardAck { seq }).await.is_err() {
                                        break;
                                    }
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
                Some(()) = async {
                    match downgrade_rx.as_mut() {
                        Some(rx) => rx.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    // SendGate backpressure: treat it as a degraded window so
                    // the adaptation controller lowers the bitrate itself —
                    // the controller stays the single owner of bitrate state
                    // (a direct encoder write in the video loop oscillated
                    // against the controller's ×1.25 recovery).
                    let now = std::time::Instant::now();
                    if last_gate_fwd.is_none_or(|t| now.duration_since(t) >= Duration::from_secs(1)) {
                        last_gate_fwd = Some(now);
                        if let Some(controller) = deps.controller.as_ref() {
                            let decision = controller.lock().unwrap().on_sample(
                                &Sample {
                                    rtt_ms: 0.0,
                                    loss_pct: 100.0,
                                    recv_kbps: 0,
                                    jitter_ms: 0.0,
                                },
                                // Zero-width window: the signal is an event and
                                // must not distort the controller's time base.
                                0,
                            );
                            if let Some(st) = decision {
                                tracing::info!(?st, "adaptation decision (send-gate backpressure)");
                                let qc = ControlMsg::QualityControl {
                                    bitrate_kbps: st.bitrate_kbps,
                                    fps: st.fps,
                                    scale: st.scale,
                                };
                                if sink.send(qc).await.is_err() {
                                    break;
                                }
                                if let Some(kf) = deps.kf_tx.as_ref() {
                                    let _ = kf.try_send(());
                                }
                                if let Some(bt) = deps.bitrate_tx.as_ref() {
                                    let _ = bt.try_send(st.bitrate_kbps);
                                }
                            }
                        }
                    }
                }
            }
        }
        // Session teardown (SessionEnd / stream close / error, including abrupt
        // network loss): release every key and button still held so the host is
        // not left with stuck input.
        if let Some(input) = deps.input.as_ref() {
            release_tracker.release_all(input.as_ref());
        }
        // Pump exit = session termination: cancel the media loops and clipboard poller.
        cancel.cancel();
        // The peer was last seen now: the quick-resume window starts at session
        // end, not at token issuance (§7.3).
        if let Some(fp) = deps.peer_fp.as_ref() {
            refresh_resume(fp);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_bucket_allows_burst_up_to_capacity() {
        let t0 = std::time::Instant::now();
        let mut b = TokenBucket::new(100.0);
        b.last = t0;
        for _ in 0..100 {
            assert!(b.try_take_at(t0), "burst within capacity should pass");
        }
        assert!(!b.try_take_at(t0), "capacity exhausted should drop");
    }

    #[test]
    fn token_bucket_refills_over_time() {
        let t0 = std::time::Instant::now();
        let mut b = TokenBucket::new(600.0);
        b.last = t0;
        for _ in 0..600 {
            assert!(b.try_take_at(t0));
        }
        assert!(!b.try_take_at(t0));
        // 100ms later: 60 tokens refilled.
        let t1 = t0 + Duration::from_millis(100);
        for _ in 0..60 {
            assert!(b.try_take_at(t1));
        }
        assert!(!b.try_take_at(t1));
    }

    #[test]
    fn token_bucket_caps_refill_at_capacity() {
        let t0 = std::time::Instant::now();
        let mut b = TokenBucket::new(10.0);
        b.last = t0;
        // Long idle: tokens saturate at capacity, no unbounded accumulation.
        let t1 = t0 + Duration::from_secs(60);
        for _ in 0..10 {
            assert!(b.try_take_at(t1));
        }
        assert!(!b.try_take_at(t1));
    }

    #[test]
    fn fit_capture_dims_preserves_aspect_ratio() {
        // 16:10 Retina panel: height is the binding constraint → 1728×1080,
        // not the old stretched 1920×1080.
        assert_eq!(fit_capture_dims(2880, 1800), (1728, 1080));
        // 16:9 exactly at the box: unchanged.
        assert_eq!(fit_capture_dims(1920, 1080), (1920, 1080));
        // 4K 16:9: width binds.
        assert_eq!(fit_capture_dims(3840, 2160), (1920, 1080));
        // Smaller than the box: never upscale.
        assert_eq!(fit_capture_dims(1280, 720), (1280, 720));
    }

    #[test]
    fn fit_capture_dims_forces_even_and_nonzero() {
        // 1440×900 @1x fits as-is (already even); an odd fit must round down.
        let (w, h) = fit_capture_dims(2881, 1801);
        assert_eq!(w % 2, 0);
        assert_eq!(h % 2, 0);
        assert!(w >= 2 && h >= 2);
    }

    #[test]
    fn pairing_begin_limiter_blocks_rapid_retries() {
        let t0 = std::time::Instant::now();
        let mut l = PairingBeginLimiter {
            last_begin: HashMap::new(),
        };
        assert!(l.allow("peer-a", t0));
        // Immediate reconnect from the same peer: blocked (no PIN popup).
        assert!(!l.allow("peer-a", t0 + Duration::from_secs(1)));
        // A different peer is unaffected.
        assert!(l.allow("peer-b", t0 + Duration::from_secs(1)));
        // After the interval the peer may pair again.
        assert!(l.allow("peer-a", t0 + PAIRING_BEGIN_MIN_INTERVAL));
    }

    fn test_ack() -> NegotiateAck {
        NegotiateAck {
            video: VideoParams {
                codec: CodecId::H264,
                max_fps: 30,
                max_bitrate_kbps: 1_000,
                initial_scale: 1.0,
            },
            audio: removent_proto::AudioParams::default(),
            resume_token: None,
        }
    }

    /// §7.3: the 30s window runs from session end ("peer last seen"), so a session
    /// longer than the window must still be resumable once it ends.
    #[test]
    fn refresh_resume_reanchors_window_to_session_end() {
        let fp = "fp-refresh-test";
        let token = [3u8; 16];
        remember_resume(fp, &token, &test_ack(), Caps::all(), None);
        // Simulate a long session: issuance is backdated to the edge of the window.
        {
            let mut store = resume_store().lock().unwrap();
            store.get_mut(fp).unwrap().issued_at = now_unix() - (RESUME_WINDOW_SECS - 1);
        }
        // The session ends now: the window re-anchors to this moment.
        refresh_resume(fp);
        {
            let store = resume_store().lock().unwrap();
            let issued_at = store.get(fp).unwrap().issued_at;
            assert!(
                now_unix().saturating_sub(issued_at) <= 1,
                "issued_at must be re-anchored to session end"
            );
        }
        assert!(validate_resume(fp, &token).is_some());
        invalidate_resume(fp);
    }

    /// §7.4: a lost rotation reply must not strand the client — the previous
    /// generation validates once, then is retired for good.
    #[test]
    fn previous_generation_token_is_tolerated_once() {
        let fp = "fp-prev-gen-test";
        let t1 = [1u8; 16];
        let t2 = [2u8; 16];
        let t3 = [4u8; 16];
        // Rotation after consuming t1: t2 is current, t1 is the tolerated previous generation.
        remember_resume(fp, &t2, &test_ack(), Caps::all(), Some(t1));
        let (_, _, matched_prev) =
            validate_resume_full(fp, &t1).expect("prev generation validates");
        assert!(matched_prev);
        let (_, _, matched_prev) = validate_resume_full(fp, &t2).expect("current token validates");
        assert!(!matched_prev);
        // Consuming the previous generation rotates without a prev: t1 is retired.
        invalidate_resume(fp);
        remember_resume(fp, &t3, &test_ack(), Caps::all(), None);
        assert!(validate_resume_full(fp, &t1).is_none());
        assert!(validate_resume_full(fp, &t2).is_none());
        invalidate_resume(fp);
    }
}
