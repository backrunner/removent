//! Sender-side backpressure gate (architecture.md §5.3 drop-before-send).
//!
//! With a write-buffer backlog of >2 frames, non-key frames are dropped; >8 frames
//! triggers a downgrade request. Stale data never enters the stream; drop semantics
//! are defined in protocol.md §7.1.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SendAction {
    Send,
    /// Drop this frame (only happens for non-key frames).
    DropFrame,
    /// Send this keyframe and request an encoder downgrade.
    SendAndDowngrade,
}

#[derive(Debug)]
pub struct SendGate {
    queued: usize,
    downgrade_requested: bool,
}

/// Threshold constants (protocol/architecture design values).
pub const SOFT_LIMIT_FRAMES: usize = 2;
pub const HARD_LIMIT_FRAMES: usize = 8;

impl Default for SendGate {
    fn default() -> Self {
        Self::new()
    }
}

impl SendGate {
    pub fn new() -> Self {
        Self {
            queued: 0,
            downgrade_requested: false,
        }
    }

    pub fn queued(&self) -> usize {
        self.queued
    }

    pub fn take_downgrade_request(&mut self) -> bool {
        std::mem::take(&mut self.downgrade_requested)
    }

    pub fn on_submit(&mut self, is_keyframe: bool) -> SendAction {
        match self.queued {
            q if q > HARD_LIMIT_FRAMES => {
                // Hard limit: only keyframes pass, everything else is dropped, and a
                // downgrade is requested.
                self.downgrade_requested = true;
                if is_keyframe {
                    self.queued += 1;
                    SendAction::SendAndDowngrade
                } else {
                    SendAction::DropFrame
                }
            }
            q if q > SOFT_LIMIT_FRAMES => {
                if is_keyframe {
                    self.queued += 1;
                    SendAction::Send
                } else {
                    // Drop the non-key frame without deepening the backlog.
                    SendAction::DropFrame
                }
            }
            _ => {
                self.queued += 1;
                SendAction::Send
            }
        }
    }

    /// Marks one submitted frame as fully written to the stream. Must be called
    /// only after the write completes (success or failure); calling it earlier
    /// makes `queued` undercount and defeats the soft/hard limits.
    pub fn on_sent(&mut self) {
        self.queued = self.queued.saturating_sub(1);
    }

    /// Reset when the connection recovers.
    pub fn reset(&mut self) {
        self.queued = 0;
        self.downgrade_requested = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_flow_sends_all() {
        let mut g = SendGate::new();
        for _ in 0..SOFT_LIMIT_FRAMES {
            assert!(matches!(g.on_submit(false), SendAction::Send));
            g.on_sent();
        }
        assert_eq!(g.queued(), 0);
    }

    #[test]
    fn soft_limit_drops_delta_keeps_keyframes() {
        let mut g = SendGate::new();
        // Build up a 3-frame backlog
        for _ in 0..3 {
            assert!(matches!(g.on_submit(false), SendAction::Send));
        }
        assert_eq!(g.queued(), 3);
        // Non-key frames are dropped without deepening the backlog
        assert!(matches!(g.on_submit(false), SendAction::DropFrame));
        assert_eq!(g.queued(), 3);
        // Keyframes pass
        assert!(matches!(g.on_submit(true), SendAction::Send));
        assert_eq!(g.queued(), 4);
    }

    #[test]
    fn hard_limit_requests_downgrade_and_only_keys_pass() {
        let mut g = SendGate::new();
        // Push the queue past the hard limit with keyframes (non-key frames are
        // already dropped at the soft limit).
        for _ in 0..HARD_LIMIT_FRAMES + 1 {
            g.on_submit(true);
        }
        assert!(matches!(g.on_submit(true), SendAction::SendAndDowngrade));
        assert!(g.take_downgrade_request());
        assert!(!g.take_downgrade_request());
        assert!(matches!(g.on_submit(false), SendAction::DropFrame));
    }

    #[test]
    fn drain_restores_normal() {
        let mut g = SendGate::new();
        for _ in 0..HARD_LIMIT_FRAMES + 4 {
            g.on_submit(false);
        }
        while g.queued() > 0 {
            g.on_sent();
        }
        assert!(matches!(g.on_submit(false), SendAction::Send));
    }
}
