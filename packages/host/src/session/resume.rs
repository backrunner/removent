use super::*;

/// Resume registry entry. The validity window is anchored to the last time the
/// peer was seen (`issued_at` is refreshed at session teardown, protocol.md §7.3),
/// and `prev_token` tolerates exactly one lost rotation reply (§7.4).
pub(super) struct ResumeEntry {
    pub(super) issued_at: u64,
    pub(super) token: [u8; 16],
    pub(super) prev_token: Option<[u8; 16]>,
    pub(super) ack: NegotiateAck,
    pub(super) caps: Caps,
}

/// fp → resume entry.
pub(super) type ResumeMap = HashMap<String, ResumeEntry>;

pub(super) fn resume_store() -> &'static std::sync::Mutex<ResumeMap> {
    static STORE: std::sync::OnceLock<std::sync::Mutex<ResumeMap>> = std::sync::OnceLock::new();
    STORE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// Evict zombie entries older than RESUME_WINDOW_SECS.
pub(super) fn sweep_expired(store: &mut ResumeMap) {
    let now = now_unix();
    store.retain(|_, e| now.saturating_sub(e.issued_at) <= RESUME_WINDOW_SECS);
}

pub(super) fn remember_resume(
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
pub(super) fn invalidate_resume(fp: &str) {
    resume_store().lock().unwrap().remove(fp);
}

/// The bool in the return value marks a match against the previous generation
/// (tolerated once for a lost rotation reply) rather than the current token.
pub(super) fn validate_resume_full(
    fp: &str,
    token: &[u8; 16],
) -> Option<(NegotiateAck, Caps, bool)> {
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

pub(super) fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------- pairing Begin rate limit ----------------

/// Minimum interval between two pairing Begins from the same peer: a client
/// reconnecting in a tight loop must not spam the host with PIN popups.
pub(super) const PAIRING_BEGIN_MIN_INTERVAL: Duration = Duration::from_secs(5);

/// Per-peer-fingerprint rate limiter for pairing Begin. Process-wide: the
/// attack it defends against reconnects per attempt, so per-connection state
/// would never trigger.
pub(super) struct PairingBeginLimiter {
    pub(super) last_begin: HashMap<String, std::time::Instant>,
}

impl PairingBeginLimiter {
    /// Whether a Begin from `peer_fp` is allowed; records the attempt.
    pub(super) fn allow(&mut self, peer_fp: &str, now: std::time::Instant) -> bool {
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

pub(super) fn pairing_begin_limiter() -> &'static std::sync::Mutex<PairingBeginLimiter> {
    static LIMITER: std::sync::OnceLock<std::sync::Mutex<PairingBeginLimiter>> =
        std::sync::OnceLock::new();
    LIMITER.get_or_init(|| {
        std::sync::Mutex::new(PairingBeginLimiter {
            last_begin: HashMap::new(),
        })
    })
}

// ---------------- main flow ----------------
