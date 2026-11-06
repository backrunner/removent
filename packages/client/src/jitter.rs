//! Bounded audio timeline. Reliable delivery can still arrive too late to play.
//! Preserve source PTS, expire old packets, and conceal short gaps without
//! freezing the playhead while transport retransmits missing bytes.

use removent_proto::audio_flags;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

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
    anchor: Option<(i64, Instant)>,
    expected_pts: Option<i64>,
    concealed: u8,
    discontinuity: bool,
    output: Option<(i64, Instant, bool)>,
    clock_window: Option<Instant>,
    best_delay_us: u64,
    arrival_group: Option<(Instant, i64)>,
    last_arrival: Option<Instant>,
    max_packet_age_us: u64,
    pub concealed_frames: u64,
    /// Stats
    pub late_dropped: u64,
    pub reordered: u64,
}

/// Output result.
#[derive(Debug, PartialEq)]
pub enum PopOutcome {
    /// A frame was popped normally.
    Packet(AudioPacketIn),
    /// A short source-time gap: conceal one 10 ms block.
    Plc(u16),
    /// Startup, or a long gap after the bounded concealment budget.
    Wait,
}

impl JitterBuffer {
    pub fn new(max_depth_ms: usize) -> Self {
        Self {
            pending: BTreeMap::new(),
            next_seq: None,
            max_depth_frames: (max_depth_ms / 10).clamp(2, 24),
            anchor: None,
            expected_pts: None,
            concealed: 0,
            discontinuity: false,
            output: None,
            concealed_frames: 0,
            clock_window: None,
            best_delay_us: u64::MAX,
            arrival_group: None,
            last_arrival: None,
            max_packet_age_us: 20_000,
            late_dropped: 0,
            reordered: 0,
        }
    }

    fn seq_before(a: u16, b: u16) -> bool {
        // Wraparound-safe comparison
        let d = (a.wrapping_sub(b)) as i16;
        d < 0
    }

    pub fn source_time(&self, pts: i64) -> Option<Instant> {
        let (base, at) = self.anchor?;
        let delta = pts.saturating_sub(base);
        if delta >= 0 {
            at.checked_add(Duration::from_micros(delta as u64))
        } else {
            at.checked_sub(Duration::from_micros(delta.unsigned_abs()))
        }
    }

    pub fn push(&mut self, pkt: AudioPacketIn) {
        self.push_at(pkt, Instant::now());
    }

    pub fn push_at(&mut self, pkt: AudioPacketIn, now: Instant) {
        if self
            .next_seq
            .is_some_and(|seq| Self::seq_before(pkt.seq, seq))
        {
            self.late_dropped += 1;
            return;
        }
        if self.pending.contains_key(&pkt.seq) {
            return;
        }
        // ScreenCaptureKit may deliver several consecutive periods together.
        // Callback cadence is not packet age: the callback, capture and playout
        // clocks have independent phases. Allow the bounded queue plus one
        // playout tick for ordinary arrivals; oversized retransmission bursts
        // still use a short catch-up deadline.
        let normal_age_us = (self.max_depth_frames.min(6) as u64 + 1) * 10_000;
        let same_group = self
            .arrival_group
            .is_some_and(|(at, _)| now.saturating_duration_since(at) < Duration::from_millis(2));
        if !same_group {
            let cadence = self
                .last_arrival
                .map_or(0, |at| now.saturating_duration_since(at).as_micros() as u64);
            self.max_packet_age_us = if (2_000..=60_000).contains(&cadence) {
                normal_age_us
            } else {
                20_000
            };
            self.arrival_group = Some((now, pkt.pts_us));
        } else if let Some((_, first_pts)) = self.arrival_group {
            let span = pkt.pts_us.saturating_sub(first_pts).unsigned_abs();
            self.max_packet_age_us = if span <= 50_000 {
                normal_age_us
            } else {
                20_000
            };
        }
        self.last_arrival = Some(now);
        if self.anchor.is_none() {
            self.anchor = Some((pkt.pts_us, now));
        }
        // A source restart must not leave the new stream behind the old clock.
        if self.expected_pts.is_some_and(|pts| {
            pkt.pts_us < pts.saturating_sub(1_000_000) || pkt.pts_us > pts.saturating_add(5_000_000)
        }) {
            self.pending.clear();
            self.next_seq = None;
            self.expected_pts = None;
            self.anchor = Some((pkt.pts_us, now));
            self.discontinuity = true;
            self.concealed = 0;
            self.clock_window = None;
            self.best_delay_us = u64::MAX;
        }
        // First arrival is an upper bound on clock offset. Earlier delivery
        // tightens it; a retransmission burst must never move it later.
        if self.source_time(pkt.pts_us).is_some_and(|at| at > now) {
            self.anchor = Some((pkt.pts_us, now));
        }
        let delay = self
            .source_time(pkt.pts_us)
            .map_or(Duration::ZERO, |at| now.saturating_duration_since(at));
        self.best_delay_us = self
            .best_delay_us
            .min(delay.as_micros().min(u64::MAX as u128) as u64);
        let window = *self.clock_window.get_or_insert(now);
        if now.saturating_duration_since(window) >= Duration::from_secs(10) {
            // Follow <=200 ppm capture clock drift using the least delayed
            // sample. Never redefine a short retransmission stall as baseline.
            if let Some((pts, at)) = self.anchor {
                self.anchor = Some((
                    pts,
                    at + Duration::from_micros(self.best_delay_us.min(2_000)),
                ));
            }
            self.clock_window = Some(now);
            self.best_delay_us = u64::MAX;
        }

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
        // Hard latency/memory cap, including wraparound. Ordinary capture batches
        // fit; only excess old audio is removed, with an explicit discontinuity.
        while self.pending.len() > self.max_depth_frames {
            let Some(next) = self.next_seq else { break };
            let Some(&victim) = self.pending.keys().min_by_key(|k| k.wrapping_sub(next)) else {
                break;
            };
            self.pending.remove(&victim);
            self.discontinuity = true;
            self.late_dropped += 1;
            if victim == next {
                self.next_seq = Some(next.wrapping_add(1));
            }
        }
    }

    fn first_seq(&self) -> Option<u16> {
        let next = self.next_seq?;
        self.pending
            .keys()
            .min_by_key(|seq| seq.wrapping_sub(next))
            .copied()
    }

    pub fn pop(&mut self) -> PopOutcome {
        self.pop_at(Instant::now())
    }

    pub fn pop_at(&mut self, now: Instant) -> PopOutcome {
        self.output = None;
        // Bound packet age independently of the downstream device prebuffer.
        // Source time continues while a reliable stream waits for retransmission.
        while let Some(seq) = self.first_seq() {
            let pkt = &self.pending[&seq];
            let at = self.source_time(pkt.pts_us).unwrap_or(now);
            if now.saturating_duration_since(at) <= Duration::from_micros(self.max_packet_age_us)
                && self
                    .expected_pts
                    .is_none_or(|pts| pkt.pts_us >= pts.saturating_sub(1_000))
            {
                break;
            }
            self.pending.remove(&seq);
            self.next_seq = Some(seq.wrapping_add(1));
            self.late_dropped += 1;
            self.discontinuity = true;
        }
        if let Some(seq) = self.first_seq() {
            let pts = self.pending[&seq].pts_us;
            if let Some(expected) = self.expected_pts
                && pts >= expected.saturating_add(5_000)
                && pts.saturating_sub(expected) <= 30_000
                && !self.discontinuity
                && self.concealed < 3
            {
                return self.conceal(expected, seq, now);
            }
            let packet = self.pending.remove(&seq).unwrap();
            let reset = std::mem::take(&mut self.discontinuity)
                || self
                    .expected_pts
                    .is_some_and(|expected| pts.saturating_sub(expected).unsigned_abs() > 1_000);
            self.output = Some((pts, self.source_time(pts).unwrap_or(now), reset));
            self.expected_pts = Some(pts.saturating_add(10_000));
            self.next_seq = Some(seq.wrapping_add(1));
            self.concealed = 0;
            return PopOutcome::Packet(packet);
        }
        if let Some(expected) = self.expected_pts
            && self.concealed < 3
            && self
                .source_time(expected)
                .is_some_and(|at| now >= at + Duration::from_micros(self.max_packet_age_us))
        {
            return self.conceal(expected, self.next_seq.unwrap_or(0), now);
        }
        PopOutcome::Wait
    }

    fn conceal(&mut self, pts: i64, seq: u16, now: Instant) -> PopOutcome {
        self.output = Some((pts, self.source_time(pts).unwrap_or(now), false));
        self.expected_pts = Some(pts.saturating_add(10_000));
        self.concealed += 1;
        self.concealed_frames += 1;
        PopOutcome::Plc(seq)
    }

    pub fn output_timing(&self) -> Option<(i64, Instant, bool)> {
        self.output
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
mod tests;
