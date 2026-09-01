//! Clipboard sync: abstract bridge + outbound polling + inbound apply (architecture.md §7.3).
//!
//! Each side holds a local [`TextClipboard`] (an NSPasteboard wrapper on real
//! machines, an in-memory implementation in tests); remote writes are prevented
//! from looping back via a suppress counter.

use removent_proto::{ClipFormat, ControlMsg};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use tokio::sync::mpsc::Sender;

/// Text clipboard abstraction.
pub trait TextClipboard: Send + Sync {
    fn change_count(&self) -> Result<u64, String>;
    fn read(&self) -> Result<String, String>;
    fn write(&self, text: &str) -> Result<(), String>;
}

/// In-memory clipboard for tests / headless environments.
#[derive(Default)]
pub struct MemoryClipboard {
    inner: std::sync::Mutex<(u64, String)>,
}

impl MemoryClipboard {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

impl TextClipboard for MemoryClipboard {
    fn change_count(&self) -> Result<u64, String> {
        Ok(self.inner.lock().unwrap().0)
    }
    fn read(&self) -> Result<String, String> {
        Ok(self.inner.lock().unwrap().1.clone())
    }
    fn write(&self, text: &str) -> Result<(), String> {
        let mut g = self.inner.lock().unwrap();
        g.0 += 1;
        g.1 = text.to_string();
        Ok(())
    }
}

/// Clipboard payloads are otherwise bounded only by the 8 MiB frame limit; cap sync
/// payloads well below that and drop oversized content instead of queueing it on the
/// control channel.
pub const MAX_CLIPBOARD_BYTES: usize = 4 * 1024 * 1024;

/// Session-level sync state.
pub struct ClipSyncState {
    pub seq: AtomicU32,
    /// Count of "from remote" writes to the local clipboard, skipped by the poller.
    pub suppress_cc: AtomicU64,
    /// Last seen local change count.
    pub last_seen_cc: AtomicU64,
}

impl ClipSyncState {
    pub fn new(local: &dyn TextClipboard) -> Self {
        let cc = local.change_count().unwrap_or(0);
        Self {
            seq: AtomicU32::new(0),
            suppress_cc: AtomicU64::new(0),
            last_seen_cc: AtomicU64::new(cc),
        }
    }
}

/// Outbound polling task: local clipboard change → ClipboardSync message to the peer.
pub fn spawn_clipboard_poller(
    local: Arc<dyn TextClipboard>,
    state: Arc<ClipSyncState>,
    cmd_tx: Sender<ControlMsg>,
    interval_ms: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(interval_ms));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let cc = match local.change_count() {
                Ok(c) => c,
                Err(_) => continue,
            };
            if cc == state.suppress_cc.load(Ordering::SeqCst) {
                state.last_seen_cc.store(cc, Ordering::SeqCst);
                continue;
            }
            if cc == state.last_seen_cc.swap(cc, Ordering::SeqCst) {
                continue;
            }
            let Ok(text) = local.read() else { continue };
            if text.is_empty() {
                continue;
            }
            if text.len() > MAX_CLIPBOARD_BYTES {
                tracing::warn!(len = text.len(), "clipboard content exceeds cap, dropping");
                continue;
            }
            let seq = state.seq.fetch_add(1, Ordering::SeqCst);
            let msg = ControlMsg::ClipboardSync {
                seq,
                format: ClipFormat::TextUtf8,
                data: text.into_bytes(),
            };
            if cmd_tx.send(msg).await.is_err() {
                return;
            }
        }
    })
}

/// Inbound apply: write remote text into the local clipboard and register the suppress count.
pub fn apply_incoming(
    local: &dyn TextClipboard,
    suppress: &AtomicU64,
    text: &[u8],
) -> Result<(), String> {
    // Enforce the same cap as the outbound poller; inbound frames can otherwise
    // carry close to the 8 MiB frame limit straight into the local clipboard.
    if text.len() > MAX_CLIPBOARD_BYTES {
        return Err(format!(
            "clipboard content exceeds cap: {} bytes",
            text.len()
        ));
    }
    let Ok(text) = std::str::from_utf8(text) else {
        return Err("non-utf8 clipboard".into());
    };
    // Read change_count BEFORE the write and suppress cc_before+1 (macOS bumps
    // changeCount by exactly 1 per write). Reading it after the write races with a
    // local copy landing in between: the suppress value would then cover that local
    // change too, swallowing it instead of syncing it back.
    let cc_before = local.change_count().ok();
    local.write(text)?;
    if let Some(cc) = cc_before {
        suppress.store(cc + 1, Ordering::SeqCst);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn poller_detects_local_change_and_emits_sync() {
        let local = MemoryClipboard::new();
        local.write("v0").unwrap();
        let state = Arc::new(ClipSyncState::new(local.as_ref()));
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let _h = spawn_clipboard_poller(local.clone(), state.clone(), tx, 20);

        local.write("hello").unwrap();
        let msg = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("poll timeout")
            .expect("channel open");
        let ControlMsg::ClipboardSync { seq, format, data } = msg else {
            panic!("wrong msg");
        };
        assert_eq!(format, ClipFormat::TextUtf8);
        assert_eq!(data, b"hello");
        assert_eq!(seq, 0);
    }

    #[test]
    fn incoming_write_sets_suppress() {
        let local = MemoryClipboard::new();
        let suppress = AtomicU64::new(0);
        apply_incoming(local.as_ref(), &suppress, b"remote-text").unwrap();
        assert_eq!(local.read().unwrap(), "remote-text");
        // Suppress = cc_before + 1 (the poller skips exactly our own write).
        assert_eq!(suppress.load(Ordering::SeqCst), 1);
        assert_eq!(local.change_count().unwrap(), 1);
        // After applying again with the correct suppress value, the poller will not echo it back.
        apply_incoming(local.as_ref(), &suppress, b"second").unwrap();
        assert_eq!(local.read().unwrap(), "second");
        assert_eq!(suppress.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn incoming_over_cap_is_rejected() {
        let local = MemoryClipboard::new();
        let suppress = AtomicU64::new(0);
        let big = vec![b'x'; MAX_CLIPBOARD_BYTES + 1];
        assert!(apply_incoming(local.as_ref(), &suppress, &big).is_err());
        // The local clipboard is left untouched.
        assert_eq!(local.read().unwrap(), "");
    }
}
