//! Clipboard sync: abstract bridge + outbound polling + inbound apply (architecture.md §7.3).
//!
//! Each side holds a local [`TextClipboard`] (an NSPasteboard wrapper on real
//! machines, an in-memory implementation in tests); remote writes are prevented
//! from looping back via a suppressed change count and serialized access.

use removent_proto::{ClipFormat, ControlMsg};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
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
    /// Serialize snapshots and remote writes, including suppression registration.
    access: Mutex<()>,
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
            access: Mutex::new(()),
            suppress_cc: AtomicU64::new(0),
            last_seen_cc: AtomicU64::new(cc),
        }
    }
    /// Capture a stable text snapshot. A paste forces a snapshot even if the
    /// content predates this connection or was already received from the peer.
    pub fn snapshot(
        &self,
        local: &dyn TextClipboard,
        force: bool,
    ) -> Result<Option<ControlMsg>, String> {
        let _access = self.access.lock().unwrap();
        for _ in 0..3 {
            let cc = local.change_count()?;
            if !force
                && (cc == self.suppress_cc.load(Ordering::SeqCst)
                    || cc == self.last_seen_cc.load(Ordering::SeqCst))
            {
                self.last_seen_cc.store(cc, Ordering::SeqCst);
                return Ok(None);
            }
            let text = local.read();
            if local.change_count()? != cc {
                continue;
            }
            self.last_seen_cc.store(cc, Ordering::SeqCst);
            let text = text?;
            if !force && text.is_empty() {
                return Ok(None);
            }
            if text.len() > MAX_CLIPBOARD_BYTES {
                return Err("clipboard content exceeds cap".into());
            }
            return Ok(Some(ControlMsg::ClipboardSync {
                seq: self.seq.fetch_add(1, Ordering::SeqCst),
                format: ClipFormat::TextUtf8,
                data: text.into_bytes(),
            }));
        }
        Err("clipboard changed during snapshot".into())
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
            let Ok(Some(msg)) = state.snapshot(local.as_ref(), false) else {
                continue;
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
    state: &ClipSyncState,
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
    let _access = state.access.lock().unwrap();
    local.write(text)?;
    // Native bridges may increment changeCount more than once. Read the actual
    // count under the poller's lock, and suppress only if this is still our text.
    // A concurrent local copy of different content must remain observable.
    let cc = local.change_count()?;
    if local.read().is_ok_and(|value| value == text) && local.change_count()? == cc {
        state.suppress_cc.store(cc, Ordering::SeqCst);
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
        let state = ClipSyncState::new(local.as_ref());
        apply_incoming(local.as_ref(), &state, b"remote-text").unwrap();
        assert_eq!(local.read().unwrap(), "remote-text");
        // The poller skips exactly our own write.
        assert_eq!(state.suppress_cc.load(Ordering::SeqCst), 1);
        assert_eq!(local.change_count().unwrap(), 1);
        // After applying again with the correct suppress value, the poller will not echo it back.
        apply_incoming(local.as_ref(), &state, b"second").unwrap();
        assert_eq!(local.read().unwrap(), "second");
        assert_eq!(state.suppress_cc.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn incoming_over_cap_is_rejected() {
        let local = MemoryClipboard::new();
        let state = ClipSyncState::new(local.as_ref());
        let big = vec![b'x'; MAX_CLIPBOARD_BYTES + 1];
        assert!(apply_incoming(local.as_ref(), &state, &big).is_err());
        // The local clipboard is left untouched.
        assert_eq!(local.read().unwrap(), "");
    }
    #[test]
    fn forced_snapshot_includes_existing_and_empty_text() {
        let local = MemoryClipboard::new();
        local.write("连接前复制\n中文 🚀").unwrap();
        let state = ClipSyncState::new(local.as_ref());
        assert!(state.snapshot(local.as_ref(), false).unwrap().is_none());
        let Some(ControlMsg::ClipboardSync { data, .. }) =
            state.snapshot(local.as_ref(), true).unwrap()
        else {
            panic!()
        };
        assert_eq!(String::from_utf8(data).unwrap(), "连接前复制\n中文 🚀");
        local.write("").unwrap();
        let Some(ControlMsg::ClipboardSync { data, .. }) =
            state.snapshot(local.as_ref(), true).unwrap()
        else {
            panic!()
        };
        assert!(data.is_empty());
    }

    struct MultiChangeClipboard {
        inner: Arc<MemoryClipboard>,
        concurrent_copy: bool,
    }
    impl TextClipboard for MultiChangeClipboard {
        fn change_count(&self) -> Result<u64, String> {
            self.inner.change_count()
        }
        fn read(&self) -> Result<String, String> {
            self.inner.read()
        }
        fn write(&self, text: &str) -> Result<(), String> {
            self.inner.write("")?;
            self.inner.write(text)?;
            if self.concurrent_copy {
                self.inner.write("local copy during remote write")?;
            }
            Ok(())
        }
    }

    #[test]
    fn suppression_uses_actual_write_count_and_preserves_concurrent_local_copy() {
        for concurrent_copy in [false, true] {
            let local = MultiChangeClipboard {
                inner: MemoryClipboard::new(),
                concurrent_copy,
            };
            let state = ClipSyncState::new(&local);
            apply_incoming(&local, &state, b"remote").unwrap();
            let snapshot = state.snapshot(&local, false).unwrap();
            if concurrent_copy {
                let Some(ControlMsg::ClipboardSync { data, .. }) = snapshot else {
                    panic!("local copy lost")
                };
                assert_eq!(data, b"local copy during remote write");
            } else {
                assert!(snapshot.is_none(), "remote text must not echo");
                assert_eq!(state.suppress_cc.load(Ordering::SeqCst), 2);
            }
        }
    }

    #[test]
    fn malformed_or_oversized_text_never_changes_clipboard() {
        let local = MemoryClipboard::new();
        local.write("keep").unwrap();
        let state = ClipSyncState::new(local.as_ref());
        assert!(apply_incoming(local.as_ref(), &state, &[0xff]).is_err());
        assert_eq!(local.read().unwrap(), "keep");
        local.write(&"x".repeat(MAX_CLIPBOARD_BYTES + 1)).unwrap();
        assert!(state.snapshot(local.as_ref(), true).is_err());
    }
}
