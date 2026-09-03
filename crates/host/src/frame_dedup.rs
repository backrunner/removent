//! Exact frame-content deduplication for the host video sender.
//!
//! VideoToolbox already performs inter-frame prediction, but submitting a
//! byte-for-byte identical capture still costs an IOSurface copy, an encoder
//! invocation, and a packet on the wire. This helper only tracks frames that
//! were successfully sent so a frame dropped by backpressure is retried.

use std::sync::Arc;

#[derive(Debug, Default)]
pub struct FrameDeduplicator {
    last_sent: Option<Arc<[u8]>>,
}

impl FrameDeduplicator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns true when `frame` differs from the last frame committed to the
    /// wire, or when a caller explicitly requires a refresh.
    pub fn should_encode(&self, frame: &[u8], force: bool) -> bool {
        force || self.last_sent.as_deref() != Some(frame)
    }

    /// Records a frame only after its encoded packet has been accepted and
    /// written. Keeping this separate from [`should_encode`] is important:
    /// backpressure drops and encoder failures must not suppress a retry.
    pub fn mark_sent(&mut self, frame: &[u8]) {
        self.last_sent = Some(Arc::from(frame));
    }

    /// Same as [`mark_sent`], but reuses an existing frame allocation shared
    /// with the capture cache and pending encoder submission.
    pub fn mark_sent_shared(&mut self, frame: Arc<[u8]>) {
        self.last_sent = Some(frame);
    }

    pub fn reset(&mut self) {
        self.last_sent = None;
    }
}

#[cfg(test)]
mod tests {
    use super::FrameDeduplicator;

    #[test]
    fn first_frame_is_encoded_and_identical_frame_is_skipped_after_commit() {
        let mut dedup = FrameDeduplicator::new();
        let frame = [1, 2, 3, 4];
        assert!(dedup.should_encode(&frame, false));
        dedup.mark_sent(&frame);
        assert!(!dedup.should_encode(&frame, false));
    }

    #[test]
    fn one_byte_change_is_encoded() {
        let mut dedup = FrameDeduplicator::new();
        dedup.mark_sent(&[1, 2, 3, 4]);
        assert!(dedup.should_encode(&[1, 2, 3, 5], false));
    }

    #[test]
    fn force_and_reset_bypass_deduplication() {
        let mut dedup = FrameDeduplicator::new();
        let frame = [9, 8, 7];
        dedup.mark_sent(&frame);
        assert!(dedup.should_encode(&frame, true));
        dedup.reset();
        assert!(dedup.should_encode(&frame, false));
    }

    #[test]
    fn uncommitted_frame_is_retried() {
        let dedup = FrameDeduplicator::new();
        let frame = [0, 1];
        assert!(dedup.should_encode(&frame, false));
        assert!(dedup.should_encode(&frame, false));
    }
}
