//! Audio packet sequencer (protocol.md §6.2 / FR-22).
//!
//! Media runs over ordered reliable QUIC streams, so reordering/loss cannot happen and
//! there is no reorder buffering: in-order packets pass through immediately. What
//! remains is sequence integrity — ordered output by seq, wraparound-safe comparisons,
//! late/duplicate drop (protocol.md §7.1) — plus a depth cap as flood protection.
//!
//! Depth is measured in frames (10ms per frame).

use removent_proto::audio_flags;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub struct AudioPacketIn {
    pub seq: u16,
    pub pts_us: i64,
    pub flags: u8,
    pub payload: Vec<u8>,
}

pub struct JitterBuffer {
    pending: BTreeMap<u16, AudioPacketIn>,
    next_seq: Option<u16>,
    max_depth_frames: usize,
    /// Stats
    pub late_dropped: u64,
    pub reordered: u64,
}

/// Output result.
#[derive(Debug, PartialEq)]
pub enum PopOutcome {
    /// A frame was popped normally.
    Packet(AudioPacketIn),
    /// The playhead packet is missing (only reachable after flood-cap eviction):
    /// skip it and conceal (carries the missing seq).
    Plc(u16),
    /// Buffer under-run, wait.
    Wait,
}

impl JitterBuffer {
    pub fn new(max_depth_ms: usize) -> Self {
        Self {
            pending: BTreeMap::new(),
            next_seq: None,
            max_depth_frames: (max_depth_ms / 10).max(2),
            late_dropped: 0,
            reordered: 0,
        }
    }

    fn seq_before(a: u16, b: u16) -> bool {
        // Wraparound-safe comparison
        let d = (a.wrapping_sub(b)) as i16;
        d < 0
    }

    pub fn push(&mut self, pkt: AudioPacketIn) {
        match self.next_seq {
            None => {
                self.next_seq = Some(pkt.seq);
                self.pending.insert(pkt.seq, pkt);
            }
            Some(next) => {
                if Self::seq_before(pkt.seq, next) {
                    // Late: the playhead has already passed; drop it.
                    self.late_dropped += 1;
                } else {
                    if !self.pending.contains_key(&pkt.seq)
                        && pkt.seq != next.wrapping_add(self.pending.len() as u16)
                    {
                        self.reordered += 1;
                    }
                    self.pending.entry(pkt.seq).or_insert(pkt);
                }
            }
        }
        // Depth-cap protection (against malicious flooding): once exceeded, evict the
        // "oldest" frame (the one nearest the playhead). Wraparound-safe: pick the one
        // with the smallest forward distance from the playhead.
        while self.pending.len() > self.max_depth_frames * 4 {
            let Some(next) = self.next_seq else { break };
            let Some(&victim) = self.pending.keys().min_by_key(|k| k.wrapping_sub(next)) else {
                break;
            };
            self.pending.remove(&victim);
            self.late_dropped += 1;
            if victim == next {
                self.next_seq = Some(next.wrapping_add(1));
            }
        }
    }

    /// Pop the next frame. Driven by the playback cadence: call once per tick. In-order
    /// packets come out immediately; there is no target-depth wait.
    pub fn pop(&mut self) -> PopOutcome {
        let Some(next) = self.next_seq else {
            return PopOutcome::Wait;
        };
        if let Some(pkt) = self.pending.remove(&next) {
            self.next_seq = Some(next.wrapping_add(1));
            return PopOutcome::Packet(pkt);
        }
        // Gap: unreachable over an ordered reliable stream; it can only arise after the
        // flood-protection cap evicted the playhead packet. Skip it immediately
        // (wraparound-safe) instead of buffering to a reorder target.
        if self.pending.keys().any(|k| Self::seq_before(next, *k)) {
            self.next_seq = Some(next.wrapping_add(1));
            return PopOutcome::Plc(next);
        }
        PopOutcome::Wait
    }

    /// Current depth (ms).
    pub fn depth_ms(&self) -> usize {
        self.pending.len() * 10
    }

    /// Current depth (frames).
    pub fn depth_frames(&self) -> usize {
        self.pending.len()
    }
}

/// Detect a DTX packet (comfort noise for silence; not fed to the decoder).
pub fn is_dtx(flags: u8) -> bool {
    flags & audio_flags::DTX != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkt(seq: u16) -> AudioPacketIn {
        AudioPacketIn {
            seq,
            pts_us: 0,
            flags: 0,
            payload: vec![1, 2, 3],
        }
    }

    #[test]
    fn in_order_flow() {
        let mut jb = JitterBuffer::new(60);
        for s in 0..5u16 {
            jb.push(pkt(s));
        }
        // In-order packets pass through immediately, in sequence.
        let got = (0..5).map(|_| jb.pop()).collect::<Vec<_>>();
        assert_eq!(
            got,
            vec![
                PopOutcome::Packet(pkt(0)),
                PopOutcome::Packet(pkt(1)),
                PopOutcome::Packet(pkt(2)),
                PopOutcome::Packet(pkt(3)),
                PopOutcome::Packet(pkt(4)),
            ]
        );
    }

    #[test]
    fn gap_skips_immediately() {
        // Gaps cannot occur over ordered reliable streams (this state is only reachable
        // after flood-cap eviction): the missing playhead packet is skipped at once
        // instead of waiting for a reorder target.
        let mut jb = JitterBuffer::new(60);
        jb.push(pkt(0));
        jb.push(pkt(2));
        assert!(matches!(jb.pop(), PopOutcome::Packet(p) if p.seq == 0));
        assert!(matches!(jb.pop(), PopOutcome::Plc(1)));
        assert!(matches!(jb.pop(), PopOutcome::Packet(p) if p.seq == 2));
    }

    #[test]
    fn large_gap_skips_to_buffered_packet() {
        let mut jb = JitterBuffer::new(60);
        jb.push(pkt(0));
        jb.push(pkt(7)); // missing 1..=6
        assert!(matches!(jb.pop(), PopOutcome::Packet(p) if p.seq == 0));
        for want in 1..=6u16 {
            assert!(
                matches!(jb.pop(), PopOutcome::Plc(s) if s == want),
                "want Plc({want})"
            );
        }
        assert!(matches!(jb.pop(), PopOutcome::Packet(p) if p.seq == 7));
    }

    #[test]
    fn wait_only_when_nothing_buffered() {
        let mut jb = JitterBuffer::new(60);
        assert!(matches!(jb.pop(), PopOutcome::Wait));
        jb.push(pkt(0));
        assert!(matches!(jb.pop(), PopOutcome::Packet(p) if p.seq == 0));
        assert!(matches!(jb.pop(), PopOutcome::Wait));
    }

    #[test]
    fn late_packets_dropped() {
        let mut jb = JitterBuffer::new(60);
        for s in 0..3u16 {
            jb.push(pkt(s));
        }
        let _ = jb.pop();
        jb.push(pkt(0)); // late
        assert_eq!(jb.late_dropped, 1);
    }

    #[test]
    fn depth_cap_evicts_oldest() {
        let mut jb = JitterBuffer::new(30); // max 3 frames, cap = 12
        for s in 0..50u16 {
            jb.push(pkt(s));
        }
        assert!(jb.pending_len_for_test() <= 12);
    }

    impl JitterBuffer {
        fn pending_len_for_test(&self) -> usize {
            self.pending.len()
        }
    }

    #[test]
    fn seq_wraparound_orders_correctly() {
        let mut jb = JitterBuffer::new(60);
        // Crossing the u16 wraparound boundary: 65534, 65535, 0, 1
        for s in [65534u16, 65535, 0, 1] {
            jb.push(pkt(s));
        }
        for want in [65534u16, 65535, 0, 1] {
            match jb.pop() {
                PopOutcome::Packet(p) => assert_eq!(p.seq, want),
                other => panic!("expected {want}, got {other:?}"),
            }
        }
    }

    #[test]
    fn wraparound_gap_skips_immediately() {
        let mut jb = JitterBuffer::new(60);
        jb.push(pkt(65535));
        jb.push(pkt(4)); // missing 0..=3, across the wraparound
        assert!(matches!(jb.pop(), PopOutcome::Packet(p) if p.seq == 65535));
        for want in 0..=3u16 {
            assert!(
                matches!(jb.pop(), PopOutcome::Plc(s) if s == want),
                "want Plc({want})"
            );
        }
        assert!(matches!(jb.pop(), PopOutcome::Packet(p) if p.seq == 4));
    }

    #[test]
    fn overflow_evicts_oldest_near_playhead_not_newest_across_wrap() {
        let mut jb = JitterBuffer::new(30); // cap = 12 frames
        // Flood many packets while the playhead is near the wraparound boundary
        for i in 0..40u16 {
            jb.push(pkt(65530u16.wrapping_add(i)));
        }
        // The playhead should advance instead of stalling; the first playable packet
        // should be near the playhead.
        let first = jb.pop();
        assert!(matches!(first, PopOutcome::Packet(_)), "{first:?}");
    }
}
