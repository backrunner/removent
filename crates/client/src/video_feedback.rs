//! Bounded, event-driven receiver measurements. A quiet static screen produces
//! no healthy samples; the host uses actual refresh writes to probe recovery.
use removent_net::RvpConnection;
use removent_proto::ControlMsg;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

#[derive(Default)]
pub(crate) struct VideoFeedback(Mutex<Window>);

#[derive(Default)]
struct Window {
    bytes: u64,
    partial_since: Option<Instant>,
    previous: Option<(i64, Instant)>,
    jitter_ms: f32,
    decode_ms: f32,
    pending: VecDeque<(i64, Instant)>,
}

impl VideoFeedback {
    pub(crate) fn begin_frame(&self) {
        self.0.lock().unwrap().partial_since = Some(Instant::now());
    }

    pub(crate) fn received(&self, pts: i64, bytes: usize) {
        self.received_at(pts, bytes, Instant::now());
    }

    fn received_at(&self, pts: i64, bytes: usize, now: Instant) {
        let mut w = self.0.lock().unwrap();
        w.bytes = w.bytes.saturating_add(bytes as u64);
        w.partial_since = None;
        if let Some((previous_pts, at)) = w.previous.replace((pts, now)) {
            let source_us = pts.saturating_sub(previous_pts);
            // Cached-screen refreshes use monotonic PTS (+1us), not a new
            // capture timestamp. Neither those nor long idle gaps are jitter.
            if (5_000..=500_000).contains(&source_us)
                && now.duration_since(at) < Duration::from_secs(2)
            {
                let arrival_ms = now.duration_since(at).as_secs_f32() * 1000.;
                w.jitter_ms = w
                    .jitter_ms
                    .max((arrival_ms - source_us as f32 / 1000.).abs());
            }
        }
    }

    pub(crate) fn decoding(&self, pts: i64) {
        let mut w = self.0.lock().unwrap();
        if w.pending.len() == 32 {
            w.pending.pop_front();
        }
        w.pending.push_back((pts, Instant::now()));
    }

    pub(crate) fn decoded(&self, pts: i64) {
        let mut w = self.0.lock().unwrap();
        if let Some(index) = w.pending.iter().position(|(id, _)| *id == pts) {
            let (_, at) = w.pending[index];
            w.decode_ms = w.decode_ms.max(at.elapsed().as_secs_f32() * 1000.);
            // Low-latency decoders are ordered; discard skipped old outputs.
            w.pending.drain(..=index);
        }
    }

    pub(crate) fn decode_failed(&self, pts: i64) {
        self.0.lock().unwrap().pending.retain(|(id, _)| *id != pts);
    }

    fn report(
        &self,
        elapsed: Duration,
        rtt_ms: f32,
        loss_pct: f32,
        now: Instant,
    ) -> Option<ControlMsg> {
        let mut w = self.0.lock().unwrap();
        let active =
            w.bytes > 0 || w.partial_since.is_some() || !w.pending.is_empty() || w.decode_ms > 0.;
        let recv_kbps = (w.bytes as f64 * 8. / elapsed.as_secs_f64().max(0.001) / 1000.)
            .min(f64::from(u32::MAX)) as u32;
        let mut jitter_ms = w.jitter_ms;
        if let Some(at) = w.partial_since {
            // Includes ordered-stream head-of-line delay on QUIC or WSS. Only
            // excess over a generous transport allowance counts as pressure.
            let allowance = (rtt_ms * 2.).max(200.);
            jitter_ms =
                jitter_ms.max((now.duration_since(at).as_secs_f32() * 1000. - allowance).max(0.));
        }
        let decode_ms = w.pending.front().map_or(w.decode_ms, |(_, at)| {
            w.decode_ms
                .max(now.duration_since(*at).as_secs_f32() * 1000.)
        });
        w.bytes = 0;
        w.jitter_ms = 0.;
        w.decode_ms = 0.;
        active.then_some(ControlMsg::StatsReport {
            rtt_ms,
            loss_pct,
            recv_kbps,
            jitter_ms,
            decode_ms,
            // Decoded output is not presentation; do not invent renderer FPS.
            render_fps: 0.,
        })
    }
}

pub(crate) async fn report_loop(
    conn: RvpConnection,
    feedback: Arc<VideoFeedback>,
    commands: mpsc::Sender<ControlMsg>,
) {
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut previous = conn.inner().stats().path;
    let mut last = Instant::now();
    loop {
        tokio::select! {
            _ = commands.closed() => break,
            _ = conn.inner().closed() => break,
            _ = tick.tick() => {
                let now = Instant::now();
                let path = conn.inner().stats().path;
                let sent = path.sent_packets.saturating_sub(previous.sent_packets);
                let lost = path.lost_packets.saturating_sub(previous.lost_packets);
                // Locally observable upstream loss. Host delivery health also
                // measures its own downstream losses and blocked writes.
                let loss = if sent == 0 { 0. } else { (lost as f32 * 100. / sent as f32).min(100.) };
                let report = feedback.report(now.duration_since(last), conn.inner().rtt().as_secs_f32() * 1000., loss, now);
                previous = path;
                last = now;
                if let Some(report) = report {
                    // Telemetry must never wait ahead of input/control messages.
                    let _ = commands.try_send(report);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn static_probes_are_not_jitter_and_idle_is_not_bandwidth_evidence() {
        let f = VideoFeedback::default();
        let t = Instant::now();
        assert!(f.report(Duration::from_millis(500), 80., 0., t).is_none());
        f.received_at(1, 1000, t);
        f.received_at(2, 1000, t + Duration::from_secs(1));
        let r = f
            .report(Duration::from_secs(1), 80., 0., t + Duration::from_secs(1))
            .unwrap();
        assert!(matches!(
            r,
            ControlMsg::StatsReport {
                jitter_ms: 0.,
                recv_kbps: 16,
                ..
            }
        ));
        assert!(
            f.report(
                Duration::from_millis(500),
                80.,
                0.,
                t + Duration::from_secs(2)
            )
            .is_none()
        );
    }

    #[test]
    fn arrival_jitter_and_partial_frame_stalls_reach_feedback() {
        let f = VideoFeedback::default();
        let t = Instant::now();
        f.received_at(0, 1000, t);
        f.received_at(33_333, 1000, t + Duration::from_millis(133));
        let r = f
            .report(
                Duration::from_millis(500),
                80.,
                0.,
                t + Duration::from_millis(500),
            )
            .unwrap();
        assert!(matches!(r, ControlMsg::StatsReport { jitter_ms, .. } if jitter_ms > 99.));
        f.0.lock().unwrap().partial_since = Some(t);
        let r = f
            .report(
                Duration::from_millis(500),
                80.,
                0.,
                t + Duration::from_millis(500),
            )
            .unwrap();
        assert!(matches!(
            r,
            ControlMsg::StatsReport {
                jitter_ms: 300.,
                recv_kbps: 0,
                ..
            }
        ));
    }

    #[test]
    fn decoder_queue_is_bounded_and_slow_decode_is_reported() {
        let f = VideoFeedback::default();
        for pts in 0..100 {
            f.decoding(pts);
        }
        assert_eq!(f.0.lock().unwrap().pending.len(), 32);
        let r = f
            .report(
                Duration::from_millis(500),
                1.,
                0.,
                Instant::now() + Duration::from_millis(100),
            )
            .unwrap();
        assert!(
            matches!(r, ControlMsg::StatsReport { decode_ms, render_fps: 0., .. } if decode_ms >= 100.)
        );
        f.decoded(99);
        assert!(f.0.lock().unwrap().pending.is_empty());
        f.decoding(100);
        f.decode_failed(100);
        assert!(f.0.lock().unwrap().pending.is_empty());
    }

    #[test]
    fn completed_decode_is_reported_even_after_its_bytes_were_reported() {
        let f = VideoFeedback::default();
        let now = Instant::now();
        f.0.lock()
            .unwrap()
            .pending
            .push_back((1, now - Duration::from_millis(100)));
        f.decoded(1);
        let report = f
            .report(Duration::from_millis(500), 1., 0., Instant::now())
            .unwrap();
        assert!(
            matches!(report, ControlMsg::StatsReport { decode_ms, recv_kbps: 0, .. } if decode_ms >= 100.)
        );
    }
}
