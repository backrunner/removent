//! Host-side session engine: admission, pairing, negotiation, and media send loops.
//!
//! Media sources are injected via tokio mpsc channels (either the SCK capture
//! implementation or a test synthesizer), so the engine does not depend on TCC
//! permissions and can be verified headless. Message sequencing: protocol.md §4–5.

use crate::admission::{AdmissionDecision, decide};
use crate::frame_dedup::FrameDeduplicator;
use crate::input_sink::InputSink;
use futures::{SinkExt, StreamExt};
use rand::RngCore;
use removent_core::{
    AdmissionMode, ClipSyncState, DeviceIdentity, PeerRecord, PeersStore, TextClipboard,
    adapt::{AdaptationController, QualityState, Sample},
};
use removent_media_codec::VideoEncoder;
use removent_net::session::send_control;
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
    pub video_tx: removent_core::latest::Sender<(Vec<u8>, i64)>,
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
    tokio::sync::watch::Sender<QualityState>,
    tokio::sync::watch::Receiver<QualityState>,
);

fn build_session_channels(bitrate_kbps: u32, fps: u8) -> SessionChannels {
    let (cmd_tx, cmd_rx) = mpsc::channel::<ControlMsg>(64);
    let (kf_tx, kf_rx) = mpsc::channel::<()>(4);
    let controller = Arc::new(std::sync::Mutex::new(AdaptationController::new(
        bitrate_kbps,
        fps,
        removent_core::QualityPreset::Auto,
    )));
    let (quality_tx, quality_rx) = tokio::sync::watch::channel(controller.lock().unwrap().state());
    (
        cmd_tx, cmd_rx, controller, kf_tx, kf_rx, quality_tx, quality_rx,
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
    /// Complete quality state; watch delivery cannot lose the latest decision.
    pub quality_tx: tokio::sync::watch::Sender<QualityState>,
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
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let item = tokio::time::timeout_at(deadline, source.next())
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
        tokio::sync::watch::Receiver<QualityState>,
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
                // File-transfer/two-way-audio/hdr remain unimplemented; the
                // software AV1 codec is available on every build and is still
                // opt-in during negotiation.
                feature_bits: removent_proto::feature_bits::SOFTWARE_AV1,
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
        let (cmd_tx, cmd_rx, controller, kf_tx, kf_rx, quality_tx, quality_rx) =
            build_session_channels(ack.video.max_bitrate_kbps, ack.video.max_fps);
        let clip_state = cfg
            .local_clip
            .clone()
            .filter(|_| peer_caps.clipboard)
            .map(|clip| spawn_clip_poller(clip, cmd_tx.clone(), &cancel));
        return Ok((
            EstablishedSession {
                peer_fp_hex: peer_fp,
                ack,
                resume_token: new_token,
                cmd_tx: cmd_tx.clone(),
                controller,
                keyframe_req_tx: kf_tx.clone(),
                quality_tx: quality_tx.clone(),
                peer_caps,
                clip_state,
                cancel,
            },
            kf_rx,
            quality_rx,
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
    let display_w = main_display.w_px;
    let display_h = main_display.h_px;
    let displays = vec![main_display];
    let selected = displays[0].id;
    let codec = preferred_video_codec(
        hello.feature_bits,
        display_w,
        display_h,
        cfg.video_bitrate_kbps,
        cfg.video_fps,
    );
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
    let (cmd_tx, cmd_rx, controller, kf_tx, kf_rx, quality_tx, quality_rx) =
        build_session_channels(ack.video.max_bitrate_kbps, ack.video.max_fps);
    let clip_state = cfg
        .local_clip
        .clone()
        .filter(|_| caps.clipboard)
        .map(|clip| spawn_clip_poller(clip, cmd_tx.clone(), &cancel));
    Ok((
        EstablishedSession {
            peer_fp_hex: peer_fp,
            ack,
            resume_token: token,
            cmd_tx,
            controller,
            keyframe_req_tx: kf_tx,
            quality_tx,
            peer_caps: caps,
            clip_state,
            cancel,
        },
        kf_rx,
        quality_rx,
        cmd_rx,
        sink,
        source,
    ))
}

/// AV1 is intentionally opt-in for remote desktop sessions. Software AV1 has
/// materially higher CPU cost and a few frames of encoder pipeline delay, so a
/// peer must advertise the decoder feature and the operator must request it via
/// `REMOVENT_VIDEO_CODEC=av1`. Any failed probe falls back to HEVC.
fn preferred_video_codec(
    peer_features: u64,
    display_w: u32,
    display_h: u32,
    bitrate_kbps: u32,
    fps: u8,
) -> CodecId {
    let requested = std::env::var("REMOVENT_VIDEO_CODEC")
        .ok()
        .is_some_and(|v| v.eq_ignore_ascii_case("av1") || v.eq_ignore_ascii_case("software-av1"));
    if !requested || peer_features & removent_proto::feature_bits::SOFTWARE_AV1 == 0 {
        return CodecId::Hevc;
    }
    let (w, h) = fit_capture_dims(display_w, display_h);
    if removent_media_codec::VideoEncoder::new(
        CodecId::Av1,
        w as usize,
        h as usize,
        bitrate_kbps,
        fps,
    )
    .is_ok()
    {
        CodecId::Av1
    } else {
        tracing::warn!("software AV1 probe failed; falling back to HEVC");
        CodecId::Hevc
    }
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

fn mark_submitted_frame_sent(
    dedup: &mut FrameDeduplicator,
    submitted_frames: &mut HashMap<i64, Arc<[u8]>>,
    pts_us: i64,
) {
    if let Some(sent_bgra) = submitted_frames.remove(&pts_us) {
        dedup.mark_sent_shared(sent_bgra);
    }
}

/// Video send loop: latest raw capture → exact deduplication → encode →
/// write to the media uni-stream.
/// Returns the JoinHandle; the loop exits immediately when `cancel` fires
/// (control-pump exit / session end). `fatal_tx` receives a SessionEnd when the
/// encoder fails fatally (init at session start or rebuild after a resolution
/// change), so the client gets an explicit end instead of a silent video hang.
/// `capture_display_id` identifies the captured display for resolution-change
/// rebuilds; `delivery` records write progress and stalls for the
/// control pump (which routes them through the adaptation controller).
/// Input geometry is acknowledged by the viewer on the control stream, so
/// encoding a new size cannot change the interpretation of queued old input.
#[allow(clippy::too_many_arguments)]
pub fn spawn_video_loop(
    mut stream: removent_net::quinn::SendStream,
    mut video_rx: removent_core::latest::Receiver<(Vec<u8>, i64)>,
    mut keyframe_req_rx: mpsc::Receiver<()>,
    mut quality_rx: tokio::sync::watch::Receiver<QualityState>,
    codec: CodecId,
    width: usize,
    height: usize,
    bitrate_kbps: u32,
    fps: u8,
    cancel: CancellationToken,
    fatal_tx: Option<mpsc::Sender<ControlMsg>>,
    capture_display_id: u64,
    delivery: Option<Arc<crate::delivery::DeliveryHealth>>,
    _input: Option<Arc<dyn InputSink>>,
) -> tokio::task::JoinHandle<()> {
    let cancel_on_exit = cancel.clone().drop_guard();
    tokio::spawn(async move {
        let _cancel_on_exit = cancel_on_exit;
        let work = async {
            let ceiling = QualityState {
                bitrate_kbps,
                fps: fps.max(1),
                scale: 1.0,
            };
            let mut quality = bounded_quality(*quality_rx.borrow_and_update(), ceiling);
            let mut source_dims = (width, height);
            let mut dims = scaled_dims(source_dims, quality.scale);
            let mut encoder =
                VideoEncoder::new(codec, dims.0, dims.1, quality.bitrate_kbps, quality.fps)
                    .map_err(|e| e.to_string())?;
            let mut quality_open = true;
            let mut capture_open = true;
            let mut next_frame = tokio::time::Instant::now();
            let mut last_raw: Option<(Arc<[u8]>, i64)> = None;
            let mut pending = false;
            let mut last_sent = tokio::time::Instant::now();
            let mut force = false;
            let mut config_changed = false;
            let mut frame_id = 0;
            let mut failed_encodes = 0;
            let mut last_pts: Option<i64> = None;
            let mut dedup = FrameDeduplicator::new();
            let mut submitted = HashMap::new();
            loop {
                if !capture_open && !pending {
                    break;
                }
                tokio::select! {
                    _ = tokio::time::sleep_until(last_sent + Duration::from_secs(1)), if quality != ceiling && last_raw.is_some() && !pending => {
                        // Probe only while degraded; cached refreshes also restore
                        // sharpness after a static screen's network recovers.
                        pending = true;
                        force = true;
                    }
                    changed = quality_rx.changed(), if quality_open => {
                        if changed.is_err() { quality_open = false; continue; }
                        let new = bounded_quality(*quality_rx.borrow_and_update(), ceiling);
                        if new == quality { continue; }
                        let new_dims = scaled_dims(source_dims, new.scale);
                        if new_dims != dims || new.fps != quality.fps {
                            // Replace only after successful construction; the first
                            // packet of this configuration must be independently decodable.
                            let replacement = VideoEncoder::new(codec, new_dims.0, new_dims.1, new.bitrate_kbps, new.fps)
                                .map_err(|e| e.to_string())?;
                            encoder = replacement;
                            dims = new_dims;
                            config_changed = true;
                        } else {
                            encoder.set_bitrate_kbps(new.bitrate_kbps).map_err(|e| e.to_string())?;
                        }
                        quality = new;
                        dedup.reset();
                        submitted.clear();
                        force = true;
                        pending = last_raw.is_some();
                        next_frame = tokio::time::Instant::now();
                    }
                    Some(()) = keyframe_req_rx.recv() => {
                        force = true;
                        pending = last_raw.is_some();
                    }
                    item = video_rx.recv(), if capture_open => {
                        let Some((bgra, pts)) = item else { capture_open = false; continue };
                        if bgra.len() != source_dims.0 * source_dims.1 * 4 {
                            let Some(new_dims) = current_capture_dims(capture_display_id)
                                .filter(|(w, h)| w * h * 4 == bgra.len()) else {
                                tracing::warn!("capture dimensions unavailable; skipping mismatched frame");
                                continue;
                            };
                            source_dims = new_dims;
                            dims = scaled_dims(source_dims, quality.scale);
                            encoder = VideoEncoder::new(codec, dims.0, dims.1, quality.bitrate_kbps, quality.fps)
                                .map_err(|e| e.to_string())?;
                            config_changed = true;
                            force = true;
                            dedup.reset();
                            submitted.clear();
                        }
                        last_raw = Some((bgra.into(), pts));
                        pending = true;
                    }
                    _ = tokio::time::sleep_until(next_frame), if pending => {
                        pending = false;
                        let (raw, pts) = last_raw.as_ref().expect("pending capture");
                        let bgra: Arc<[u8]> = if dims == source_dims { raw.clone() } else {
                            removent_media_codec::scale::scale_bgra(raw, source_dims, dims)?.into()
                        };
                        if !dedup.should_encode(&bgra, force) {
                            next_frame = tokio::time::Instant::now() + frame_interval(quality.fps);
                            continue;
                        }
                        if force { encoder.request_keyframe(); }
                        let pts = last_pts.map_or(*pts, |last| (*pts).max(last.saturating_add(1)));
                        last_pts = Some(pts);
                        submitted.insert(pts, bgra);
                        if submitted.len() > 8 && let Some(oldest) = submitted.keys().copied().min() {
                            submitted.remove(&oldest);
                        }
                        let frames = match encoder.encode_bgra(submitted.get(&pts).unwrap(), pts) {
                            Ok(frames) => frames,
                            Err(e) => {
                                tracing::warn!(err=%e, "video encode failed; forcing refresh");
                                dedup.reset();
                                submitted.clear();
                                force = true;
                                pending = true;
                                failed_encodes += 1;
                                if failed_encodes >= 8 { return Err(format!("encoder repeatedly failed: {e}")); }
                                next_frame = tokio::time::Instant::now() + frame_interval(quality.fps);
                                continue;
                            }
                        };
                        let mut sent = false;
                        for ef in frames {
                            frame_id += 1;
                            let mut flags = if ef.keyframe { removent_proto::video_flags::KEYFRAME } else { 0 };
                            if ef.keyframe && config_changed {
                                flags |= removent_proto::video_flags::CONFIG_CHANGED;
                                config_changed = false;
                            }
                            let hdr = removent_proto::VideoFrameHeader {
                                frame_id, pts_us: ef.pts_us, flags, codec,
                                width: dims.0 as u16, height: dims.1 as u16,
                                payload_len: annexb_len_of(&ef),
                            };
                            let wire = build_video_frame(&hdr, &ef.data);
                            if let Some(health) = &delivery { health.begin_write(); }
                            let result = tokio::time::timeout(Duration::from_secs(10), stream.write_all(&wire)).await;
                            if let Some(health) = &delivery { health.end_write(); }
                            if !matches!(result, Ok(Ok(()))) { return Ok::<(), String>(()); }
                            sent = true;
                            mark_submitted_frame_sent(&mut dedup, &mut submitted, ef.pts_us);
                        }
                        if sent {
                            failed_encodes = 0;
                            force = false;
                            last_sent = tokio::time::Instant::now();
                        } else {
                            // Software encoders can delay their first packet; a
                            // static capture must still finish a requested refresh.
                            pending = true;
                            failed_encodes += 1;
                            if failed_encodes >= 8 { return Err("encoder produced no frames after repeated submissions".into()); }
                        }
                        if codec != CodecId::Av1 { submitted.remove(&pts); }
                        // Schedule from completion: a stalled writer must never
                        // burst to catch up with elapsed frame deadlines.
                        next_frame = tokio::time::Instant::now() + frame_interval(quality.fps);
                    }
                }
            }
            Ok::<(), String>(())
        };
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {},
            result = work => {
                if let Err(e) = result {
                    tracing::error!(err=%e, "video pipeline failed");
                    if let Some(tx) = fatal_tx {
                        let _ = tokio::time::timeout(Duration::from_secs(1), tx.send(ControlMsg::SessionEnd {
                            reason: removent_proto::EndReason::InternalError,
                        })).await;
                        let _ = tokio::time::timeout(Duration::from_secs(1), cancel.cancelled()).await;
                    }
                }
            },
        }
    })
}

fn bounded_quality(value: QualityState, ceiling: QualityState) -> QualityState {
    QualityState {
        bitrate_kbps: value.bitrate_kbps.clamp(1, ceiling.bitrate_kbps.max(1)),
        fps: value.fps.clamp(1, ceiling.fps.max(1)),
        scale: if value.scale.is_finite() {
            value.scale.clamp(0.5, 1.0)
        } else {
            1.0
        },
    }
}

fn scaled_dims((w, h): (usize, usize), scale: f32) -> (usize, usize) {
    (
        ((w as f32 * scale) as usize & !1).max(2),
        ((h as f32 * scale) as usize & !1).max(2),
    )
}

fn frame_interval(fps: u8) -> Duration {
    Duration::from_secs_f64(1.0 / f64::from(fps.max(1)))
}

/// Coalesce raw audio captures. Snapshot the queue length so a fast producer
/// cannot keep this drain running indefinitely.
fn newest_queued<T>(mut newest: T, rx: &mut mpsc::Receiver<T>) -> T {
    for _ in 0..rx.len() {
        match rx.try_recv() {
            Ok(frame) => newest = frame,
            Err(_) => break,
        }
    }
    newest
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
    let cancel_on_exit = cancel.clone().drop_guard();
    tokio::spawn(async move {
        let _cancel_on_exit = cancel_on_exit;
        let work = async {
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
                        newest_queued(frame, &mut audio_rx)
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
                if !matches!(
                    tokio::time::timeout(Duration::from_secs(10), stream.write_all(&wire)).await,
                    Ok(Ok(()))
                ) {
                    return;
                }
            }
        };
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {},
            _ = work => {},
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
    pub quality_tx: Option<tokio::sync::watch::Sender<QualityState>>,
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
    /// Actual write progress, stalls and QUIC loss sampled by the controller.
    pub delivery: Option<Arc<crate::delivery::DeliveryHealth>>,
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

fn sample_elapsed_ms(last: &mut std::time::Instant) -> u64 {
    let now = std::time::Instant::now();
    let elapsed = now.duration_since(*last).as_millis().min(u64::MAX as u128) as u64;
    *last = now;
    elapsed
}

/// Input cleanup must also run when the owning service aborts this task.
struct ControlCleanup {
    tracker: crate::input_sink::InputReleaseTracker,
    input: Option<Arc<dyn InputSink>>,
    cancel: CancellationToken,
    peer_fp: Option<String>,
}

impl Drop for ControlCleanup {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(input) = &self.input {
            self.tracker.release_all(input.as_ref());
        }
        if let Some(fp) = &self.peer_fp {
            refresh_resume(fp);
        }
    }
}

/// Control pump: inbound handling (Pong/keyframe/stats adaptation/input
/// injection/clipboard application) plus outbound command forwarding. The only task
/// holding the sink for the duration of the session.
pub fn spawn_control_pump(
    mut source: ControlSource,
    mut sink: ControlSink,
    deps: ControlPumpDeps,
    mut cmd_rx: mpsc::Receiver<ControlMsg>,
) -> tokio::task::JoinHandle<()> {
    if let Some(controller) = &deps.controller {
        controller
            .lock()
            .unwrap()
            .set_scale_enabled(!deps.caps.input);
    }
    let mut cleanup = ControlCleanup {
        tracker: Default::default(),
        input: deps.input.clone(),
        cancel: deps.cancel.clone(),
        peer_fp: deps.peer_fp.clone(),
    };
    tokio::spawn(async move {
        let cancel = deps.cancel.clone();
        let work = async {
            let mut last_kf: Option<std::time::Instant> = None;
            let mut input_bucket = TokenBucket::new(INPUT_RATE_LIMIT_PER_SEC);
            let mut remote_sample: Option<(std::time::Instant, Sample)> = None;
            let mut input_geometry: Option<(u32, u32)> = None;
            let mut sample_tick = tokio::time::interval(Duration::from_millis(250));
            sample_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // Injection failures (e.g. Accessibility permission revoked mid-session)
            // are logged at most once per 5s to avoid flooding at 600 events/s.
            let mut last_inject_warn: Option<std::time::Instant> = None;
            let mut note_inject_err = |what: &str, e: String| {
                if last_inject_warn.is_none_or(|t| t.elapsed() >= Duration::from_secs(5)) {
                    tracing::warn!(%what, err=%e, "input injection failed (accessibility permission revoked?)");
                    last_inject_warn = Some(std::time::Instant::now());
                }
            };
            // Both network reports and local write-pressure samples share one
            // monotonic clock. Event-only samples must advance hysteresis too.
            let mut last_sample_at = std::time::Instant::now();
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
                                    if send_control(&mut sink, ControlMsg::Pong { ts_us }).await.is_err() {
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
                                    remote_sample = Some((std::time::Instant::now(), Sample { rtt_ms, loss_pct, recv_kbps, jitter_ms }));
                                }
                                ControlMsg::FrameGeometry { width, height } => {
                                    if (2..=MAX_CAPTURE_W).contains(&width) && (2..=MAX_CAPTURE_H).contains(&height) {
                                        if let Some(old) = input_geometry.replace((width, height)) {
                                            cleanup.tracker.rescale_position(old, (width, height));
                                        }
                                        if let Some(input) = &deps.input { input.set_capture_dims(width, height); }
                                        if let Some(controller) = &deps.controller {
                                            controller.lock().unwrap().set_scale_enabled(true);
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
                                    cleanup.tracker.note_mouse(display_id, x_px, y_px, kind);
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
                                    cleanup.tracker.note_key(vk_code, modifiers, kind);
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
                                ControlMsg::ClipboardSync { seq, format, data } => {
                                    if !deps.caps.clipboard {
                                        tracing::warn!("clipboard sync from client without clipboard cap, ignored");
                                        // Ack receipt even when this peer did not negotiate
                                        // clipboard application; otherwise a sender waiting
                                        // for confirmation can stall indefinitely.
                                        if send_control(&mut sink, ControlMsg::ClipboardAck { seq }).await.is_err() {
                                            break;
                                        }
                                        continue;
                                    }
                                    if format == removent_proto::ClipFormat::TextUtf8
                                        && let Some(clip) = deps.local_clip.as_ref() {
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
                                    }
                                    // Receipt is acknowledged independently of whether a local
                                    // clipboard implementation exists.
                                    if send_control(&mut sink, ControlMsg::ClipboardAck { seq }).await.is_err() {
                                        break;
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
                                let _ = send_control(&mut sink, msg).await;
                                break;
                            }
                            None => break,
                            Some(msg) => {
                                if send_control(&mut sink, msg).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    _ = sample_tick.tick() => {
                        let elapsed = sample_elapsed_ms(&mut last_sample_at);
                        let local = deps.delivery.as_ref().and_then(|health| health.sample());
                        let remote = remote_sample.as_ref().filter(|(at, _)| at.elapsed() <= Duration::from_millis(750)).map(|(_, sample)| *sample);
                        let sample = match (local, remote) {
                            (Some(healthy), remote) => {
                                let mut sample = remote.unwrap_or(Sample { rtt_ms: 0., loss_pct: 0., recv_kbps: 0, jitter_ms: 0. });
                                if !healthy { sample.jitter_ms = sample.jitter_ms.max(100.); }
                                Some(sample)
                            }
                            (None, remote) => remote,
                        };
                        let decision = deps.controller.as_ref().and_then(|controller| {
                            let mut controller = controller.lock().unwrap();
                            if elapsed > 1000 || sample.is_none() { controller.pause_recovery(); }
                            sample.and_then(|sample| controller.on_sample(&sample, elapsed.min(1000)))
                        });
                        if let Some(st) = decision {
                            if let Some(tx) = &deps.quality_tx { tx.send_replace(st); }
                            if send_control(&mut sink, ControlMsg::QualityControl {
                                bitrate_kbps: st.bitrate_kbps, fps: st.fps, scale: st.scale,
                            }).await.is_err() { break; }
                        }
                    }
                }
            }
        };
        // A send can block on QUIC flow control even while keepalives succeed.
        // Cancel the entire operation, including an in-progress sink.send.
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {},
            _ = work => {},
        }
        drop(cleanup);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn adaptive_recovery_uses_real_static_refreshes_and_updates_wire_size() {
        let (_endpoints, client, host) = quic_pair().await;
        let (send, recv_client) = client.inner().open_bi().await.unwrap();
        let mut client_sink = ControlSink::new(send, removent_net::ControlCodec);
        client_sink
            .send(ControlMsg::Ping { ts_us: 0 })
            .await
            .unwrap();
        let (send, recv) = host.inner().accept_bi().await.unwrap();
        let mut source = ControlSource::new(recv, removent_net::ControlCodec);
        source.next().await.unwrap().unwrap();
        let mut controller =
            AdaptationController::new(3000, 30, removent_core::QualityPreset::Auto);
        for _ in 0..20 {
            controller.on_sample(
                &Sample {
                    rtt_ms: 0.,
                    loss_pct: 10.,
                    recv_kbps: 0,
                    jitter_ms: 100.,
                },
                2500,
            );
        }
        assert_eq!(controller.state().scale, 0.5);
        let (quality_tx, quality_rx) = tokio::sync::watch::channel(controller.state());
        let health = Arc::new(crate::delivery::DeliveryHealth::new(host.clone()));
        let (cmd_tx, cmd_rx) = mpsc::channel(4);
        let cancel = CancellationToken::new();
        let _stop_on_drop = cancel.clone().drop_guard();
        let control = spawn_control_pump(
            source,
            ControlSink::new(send, removent_net::ControlCodec),
            ControlPumpDeps {
                kf_tx: None,
                controller: Some(Arc::new(std::sync::Mutex::new(controller))),
                window_ms: 250,
                input: None,
                local_clip: None,
                quality_tx: Some(quality_tx),
                caps: Caps {
                    input: false,
                    ..Caps::all()
                },
                clip_state: None,
                cancel: cancel.clone(),
                peer_fp: None,
                delivery: Some(health.clone()),
            },
            cmd_rx,
        );
        let (frames, rx) = removent_core::latest::channel();
        let (_kf, kf_rx) = mpsc::channel(4);
        let video = spawn_video_loop(
            host.open_media_stream().await.unwrap(),
            rx,
            kf_rx,
            quality_rx,
            CodecId::H264,
            320,
            240,
            3000,
            30,
            cancel.clone(),
            None,
            0,
            Some(health),
            None,
        );
        frames
            .send(([40, 80, 160, 255].repeat(320 * 240), 0))
            .unwrap();
        let mut stream = client.accept_media_stream().await.unwrap();
        let (first, _) = video_packet(&mut stream).await;
        assert_eq!((first.width, first.height), (160, 120));
        let start = tokio::time::Instant::now();
        let recovered = tokio::time::timeout(Duration::from_secs(9), async {
            loop {
                let (header, _) = video_packet(&mut stream).await;
                if header.width > first.width {
                    break header;
                }
            }
        })
        .await
        .unwrap();
        assert!(
            start.elapsed() >= Duration::from_secs(4),
            "must not immediately upgrade from one write"
        );
        assert_eq!((recovered.width, recovered.height), (240, 180));
        assert!(recovered.is_keyframe() && recovered.config_changed());
        let mut replies = ControlSource::new(recv_client, removent_net::ControlCodec);
        let reply = tokio::time::timeout(Duration::from_secs(1), replies.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(
            matches!(reply, ControlItem::Msg(msg) if matches!(*msg, ControlMsg::QualityControl { scale: 0.75, fps: 15, bitrate_kbps: 1000 }))
        );
        drop(cmd_tx);
        cancel.cancel();
        video.await.unwrap();
        control.await.unwrap();
    }

    #[derive(Default)]
    struct GeometryInput {
        dims: std::sync::Mutex<(u32, u32)>,
        points: std::sync::Mutex<Vec<(f32, f32)>>,
    }
    impl InputSink for GeometryInput {
        fn set_capture_dims(&self, w: u32, h: u32) {
            *self.dims.lock().unwrap() = (w, h);
        }
        fn mouse(&self, _: u64, x: f32, y: f32, _: u8, _: MouseKind) -> Result<(), String> {
            let (w, h) = *self.dims.lock().unwrap();
            self.points
                .lock()
                .unwrap()
                .push((x / w as f32, y / h as f32));
            Ok(())
        }
        fn key(
            &self,
            _: u16,
            _: removent_proto::KeyModifiers,
            _: KeyKind,
            _: Option<char>,
        ) -> Result<(), String> {
            Ok(())
        }
        fn scroll(&self, _: u64, _: f32, _: f32, _: ScrollPhase) -> Result<(), String> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn displayed_geometry_orders_with_mouse_input_across_resize() {
        let (_endpoints, client, host) = quic_pair().await;
        let (send, _recv) = client.inner().open_bi().await.unwrap();
        let mut sink = ControlSink::new(send, removent_net::ControlCodec);
        for (width, height) in [(320, 240), (160, 120), (320, 240)] {
            sink.send(ControlMsg::FrameGeometry { width, height })
                .await
                .unwrap();
            sink.send(ControlMsg::MouseEvent {
                display_id: 0,
                x_px: width as f32 / 2.,
                y_px: height as f32 / 2.,
                buttons: 0,
                kind: MouseKind::Moved,
            })
            .await
            .unwrap();
        }
        sink.send(ControlMsg::SessionEnd {
            reason: removent_proto::EndReason::ClientClosed,
        })
        .await
        .unwrap();
        let (send, recv) = host.inner().accept_bi().await.unwrap();
        let input = Arc::new(GeometryInput::default());
        let (_tx, rx) = mpsc::channel(4);
        let cancel = CancellationToken::new();
        let task = spawn_control_pump(
            ControlSource::new(recv, removent_net::ControlCodec),
            ControlSink::new(send, removent_net::ControlCodec),
            ControlPumpDeps {
                kf_tx: None,
                controller: None,
                window_ms: 250,
                input: Some(input.clone()),
                local_clip: None,
                quality_tx: None,
                caps: Caps::all(),
                clip_state: None,
                cancel,
                peer_fp: None,
                delivery: None,
            },
            rx,
        );
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(*input.points.lock().unwrap(), vec![(0.5, 0.5); 3]);
    }

    async fn video_packet(
        stream: &mut removent_net::quinn::RecvStream,
    ) -> (removent_proto::VideoFrameHeader, Vec<u8>) {
        tokio::time::timeout(Duration::from_secs(3), async {
            let mut header = [0; 27];
            stream.read_exact(&mut header).await.unwrap();
            let (header, _) = removent_proto::parse_video_header(&header).unwrap();
            let mut payload = vec![0; header.payload_len as usize];
            stream.read_exact(&mut payload).await.unwrap();
            (header, payload)
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn dynamic_quality_refreshes_static_frame_and_decodes_each_size() {
        for codec in [CodecId::H264, CodecId::Hevc, CodecId::Av1] {
            let (_endpoints, client, host) = quic_pair().await;
            let max = QualityState {
                bitrate_kbps: 3000,
                fps: 30,
                scale: 1.0,
            };
            let (tx, rx) = removent_core::latest::channel();
            let (_kf_tx, kf_rx) = mpsc::channel(4);
            let (quality_tx, quality_rx) = tokio::sync::watch::channel(max);
            let cancel = CancellationToken::new();
            let _stop_on_drop = cancel.clone().drop_guard();
            let task = spawn_video_loop(
                host.open_media_stream().await.unwrap(),
                rx,
                kf_rx,
                quality_rx,
                codec,
                320,
                240,
                3000,
                30,
                cancel.clone(),
                None,
                0,
                None,
                None,
            );
            tx.send(([40, 90, 160, 255].repeat(320 * 240), 10)).unwrap();
            let mut stream = client.accept_media_stream().await.unwrap();
            let (initial, _) = video_packet(&mut stream).await;
            assert_eq!((initial.width, initial.height), (320, 240));
            let mut previous_pts = initial.pts_us;
            for quality in [
                QualityState {
                    bitrate_kbps: 1000,
                    fps: 15,
                    scale: 0.5,
                },
                QualityState {
                    bitrate_kbps: 1000,
                    fps: 15,
                    scale: 0.75,
                },
                QualityState {
                    bitrate_kbps: 1000,
                    fps: 15,
                    scale: 1.0,
                },
                max,
            ] {
                quality_tx.send_replace(quality);
                // No new captures: quality changes must refresh the cached screen.
                let (header, payload) = video_packet(&mut stream).await;
                let dims = scaled_dims((320, 240), quality.scale);
                assert_eq!((header.width as usize, header.height as usize), dims);
                assert!(header.is_keyframe() && header.config_changed());
                assert!(header.pts_us > previous_pts);
                previous_pts = header.pts_us;
                let params = if codec == CodecId::Av1 {
                    Vec::new()
                } else {
                    removent_media_codec::extract_param_sets(&payload, codec == CodecId::Hevc)
                };
                let decoder =
                    removent_media_codec::VideoDecoder::new(codec, dims.0, dims.1, &params)
                        .unwrap();
                decoder.decode_annexb(&payload, header.pts_us).unwrap();
                let decoded = tokio::time::timeout(Duration::from_secs(2), async {
                    loop {
                        if let Some(frame) = decoder.try_recv_decoded() {
                            break frame;
                        }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                })
                .await
                .unwrap();
                assert_eq!(decoded.data.len(), dims.0 * dims.1 * 4);
            }
            // A newer watch value replaces queued stale decisions, including recovery.
            quality_tx.send_replace(QualityState {
                fps: 15,
                scale: 0.5,
                ..max
            });
            quality_tx.send_replace(max);
            cancel.cancel();
            tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap();
        }
    }

    #[tokio::test]
    async fn dynamic_fps_limits_real_wire_frame_rate_under_capture_flood() {
        let (_endpoints, client, host) = quic_pair().await;
        let max = QualityState {
            bitrate_kbps: 3000,
            fps: 60,
            scale: 1.0,
        };
        let (tx, rx) = removent_core::latest::channel();
        let (_kf_tx, kf_rx) = mpsc::channel(4);
        let (quality_tx, quality_rx) = tokio::sync::watch::channel(max);
        let cancel = CancellationToken::new();
        let _stop_on_drop = cancel.clone().drop_guard();
        let task = spawn_video_loop(
            host.open_media_stream().await.unwrap(),
            rx,
            kf_rx,
            quality_rx,
            CodecId::H264,
            64,
            64,
            3000,
            60,
            cancel.clone(),
            None,
            0,
            None,
            None,
        );
        tx.send(([0, 80, 170, 255].repeat(64 * 64), 0)).unwrap();
        let mut stream = client.accept_media_stream().await.unwrap();
        video_packet(&mut stream).await;
        quality_tx.send_replace(QualityState { fps: 15, ..max });
        video_packet(&mut stream).await; // configuration keyframe
        let stop = cancel.clone();
        let producer = tokio::spawn(async move {
            let mut pts = 1;
            loop {
                tokio::select! {
                    _ = stop.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_millis(1)) => {
                        if tx.send(([pts as u8, 80, 170, 255].repeat(64 * 64), pts)).is_err() { break; }
                        pts += 1;
                    }
                }
            }
        });
        let start = tokio::time::Instant::now();
        // Observe ten complete frames; at 15 fps they cannot arrive as a burst.
        for _ in 0..10 {
            video_packet(&mut stream).await;
        }
        assert!(start.elapsed() >= Duration::from_millis(550));
        cancel.cancel();
        task.await.unwrap();
        producer.await.unwrap();
    }

    #[tokio::test]
    async fn raw_backlog_coalesces_to_latest_without_dropping_future_capture() {
        let (tx, mut rx) = mpsc::channel(4);
        for frame in 1..=4 {
            tx.send(frame).await.unwrap();
        }
        let first = rx.recv().await.unwrap();
        assert_eq!(newest_queued(first, &mut rx), 4);
        assert!(rx.is_empty());
        tx.send(5).await.unwrap();
        assert_eq!(rx.recv().await, Some(5));
    }

    async fn quic_pair() -> (
        [removent_net::quinn::Endpoint; 2],
        RvpConnection,
        RvpConnection,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let identity = removent_core::identity::load_or_create(
            &removent_core::DataPaths {
                root: dir.path().to_owned(),
            },
            "stall-test",
        )
        .unwrap();
        let (client, _) = removent_net::make_client_endpoint(
            "127.0.0.1:0".parse().unwrap(),
            &identity,
            removent_net::PinState::new([], true),
        )
        .unwrap();
        let (host, _) = removent_net::make_server_endpoint(
            "127.0.0.1:0".parse().unwrap(),
            &identity,
            removent_net::PinState::new([], true),
        )
        .unwrap();
        let (c, h) = tokio::join!(
            client
                .connect(host.local_addr().unwrap(), "removent")
                .unwrap(),
            async { host.accept().await.unwrap().await }
        );
        (
            [client, host],
            RvpConnection::new(c.unwrap()),
            RvpConnection::new(h.unwrap()),
        )
    }

    #[tokio::test]
    async fn blocked_control_write_releases_inputs_on_cancel_abort_and_timeout() {
        use crate::input_sink::{RecordedInput, RecorderInputSink};
        use removent_proto::{KeyKind, KeyModifiers};
        for mode in 0..3 {
            let (_endpoints, client, host) = quic_pair().await;
            let (send, _recv) = client.inner().open_bi().await.unwrap();
            let mut client_sink = ControlSink::new(send, removent_net::ControlCodec);
            client_sink
                .send(ControlMsg::KeyEvent {
                    vk_code: 0,
                    modifiers: KeyModifiers::empty(),
                    kind: KeyKind::Down,
                    unicode: None,
                })
                .await
                .unwrap();
            let (send, recv) = host.inner().accept_bi().await.unwrap();
            // Exhaust the sender's write budget while QUIC itself remains live.
            host.inner().set_send_window(0);
            let recorder = Arc::new(RecorderInputSink::default());
            let cancel = CancellationToken::new();
            let (tx, rx) = mpsc::channel(4);
            let mut task = spawn_control_pump(
                ControlSource::new(recv, removent_net::ControlCodec),
                ControlSink::new(send, removent_net::ControlCodec),
                ControlPumpDeps {
                    kf_tx: None,
                    controller: None,
                    window_ms: 250,
                    input: Some(recorder.clone()),
                    local_clip: None,
                    quality_tx: None,
                    caps: Caps::all(),
                    clip_state: None,
                    cancel: cancel.clone(),
                    peer_fp: None,
                    delivery: None,
                },
                rx,
            );
            tokio::time::timeout(Duration::from_secs(2), async {
                while recorder.events.lock().unwrap().is_empty() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            tx.send(ControlMsg::Ping { ts_us: 1 }).await.unwrap();
            tokio::time::timeout(Duration::from_secs(2), async {
                while tx.capacity() != 4 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert!(!task.is_finished());
            match mode {
                0 => cancel.cancel(),
                1 => task.abort(),
                _ => {} // Keep the connection live; the write deadline must fire.
            }
            let result = tokio::time::timeout(Duration::from_secs(12), &mut task)
                .await
                .unwrap();
            assert_eq!(result.is_err(), mode == 1);
            assert!(cancel.is_cancelled());
            let events = recorder.events.lock().unwrap();
            assert_eq!(events.len(), 2);
            assert!(matches!(
                events[1],
                RecordedInput::Key {
                    kind: KeyKind::Up,
                    ..
                }
            ));
        }
    }

    #[tokio::test]
    async fn blocked_audio_write_exits_on_cancel() {
        let (_endpoints, _client, host) = quic_pair().await;
        host.inner().set_send_window(0);
        let stream = host.open_media_stream().await.unwrap();
        let (tx, rx) = mpsc::channel(1);
        let cancel = CancellationToken::new();
        let mut task = spawn_audio_loop(stream, rx, 96, cancel.clone());
        tx.send(removent_media_capture::AudioFrame {
            samples: vec![0; 960],
            pts_micros: 1,
        })
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while tx.capacity() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!task.is_finished());
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(1), &mut task)
            .await
            .unwrap()
            .unwrap();
    }

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
    fn delayed_packet_commits_its_own_submitted_pixels() {
        let frame_a: Arc<[u8]> = Arc::from([1, 2, 3, 4]);
        let frame_b: Arc<[u8]> = Arc::from([5, 6, 7, 8]);
        let mut submitted = HashMap::new();
        submitted.insert(10, frame_a.clone());
        submitted.insert(20, frame_b.clone());
        let mut dedup = FrameDeduplicator::new();

        // AV1 can return A while the encode call is currently submitting B.
        mark_submitted_frame_sent(&mut dedup, &mut submitted, 10);
        assert!(!dedup.should_encode(&frame_a, false));
        assert!(dedup.should_encode(&frame_b, false));

        mark_submitted_frame_sent(&mut dedup, &mut submitted, 20);
        assert!(!dedup.should_encode(&frame_b, false));
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
