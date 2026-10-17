//! Measurements from actual media writes, shared with the control pump.
use removent_net::RvpConnection;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct DeliveryHealth {
    window: Mutex<Window>,
    conn: Option<RvpConnection>,
}

#[derive(Default)]
struct Window {
    started: Option<Instant>,
    last_completed: Option<(Instant, bool)>,
    slow: bool,
    sent_packets: u64,
    lost_packets: u64,
    capture_dims: Option<(u32, u32)>,
    encode_load: f64,
    last_encoded_at: Option<Instant>,
}

impl DeliveryHealth {
    pub fn new(conn: RvpConnection) -> Self {
        let path = conn.inner().stats().path;
        Self {
            window: Mutex::new(Window {
                sent_packets: path.sent_packets,
                lost_packets: path.lost_packets,
                capture_dims: None,
                encode_load: 0.,
                ..Default::default()
            }),
            conn: Some(conn),
        }
    }

    pub fn begin_write(&self) {
        self.window.lock().unwrap().started = Some(Instant::now());
    }

    pub fn end_write(&self) {
        let mut window = self.window.lock().unwrap();
        if let Some(started) = window.started.take() {
            let healthy = started.elapsed() < Duration::from_millis(100);
            window.last_completed = Some((Instant::now(), healthy));
            window.slow |= !healthy;
        }
    }

    pub fn encoder_limited(&self) {
        self.window.lock().unwrap().slow = true;
    }

    pub fn encoded(&self, elapsed: Duration, frame_budget: Duration) {
        self.encoded_at(elapsed, frame_budget, Instant::now());
    }

    fn encoded_at(&self, elapsed: Duration, frame_budget: Duration, now: Instant) {
        let mut window = self.window.lock().unwrap();
        // An idle screen may produce one cold encoder refresh each second.
        // Such isolated work cannot overload a 60 FPS stream; do not preserve
        // an old busy-window average across that idle interval.
        if window
            .last_encoded_at
            .replace(now)
            .is_none_or(|at| now.duration_since(at) > Duration::from_millis(250))
        {
            window.encode_load = 0.;
        }
        let load = elapsed.as_secs_f64() / frame_budget.as_secs_f64().max(0.001);
        window.encode_load += (load - window.encode_load) / 16.;
        window.slow |= window.encode_load > 1.2;
    }

    pub fn set_capture_dims(&self, width: u32, height: u32) {
        self.window.lock().unwrap().capture_dims = Some((width, height));
    }

    pub fn capture_dims(&self) -> Option<(u32, u32)> {
        self.window.lock().unwrap().capture_dims
    }

    pub fn frame_work_load(&self) -> f64 {
        self.window.lock().unwrap().encode_load
    }

    /// A fixed large send budget hides seconds of stale media on narrow links.
    /// Track the selected bitrate and base RTT, rather than a queue-inflated RTT.
    /// Keep a small burst allowance and bound fast/high-latency paths as well.
    pub fn update_send_budget(&self, bitrate_kbps: u32) {
        if let Some(conn) = &self.conn {
            let rtt = conn.inner().stats().path.min_rtt;
            conn.inner().set_send_window(send_budget(bitrate_kbps, rtt));
        }
    }

    /// None means no fresh evidence, not a healthy network. A degraded static
    /// screen sends a cached refresh once per second to obtain fresh evidence.
    pub fn sample(&self) -> Option<bool> {
        let mut window = self.window.lock().unwrap();
        let loss = self.conn.as_ref().is_some_and(|conn| {
            let path = conn.inner().stats().path;
            let sent = path.sent_packets.saturating_sub(window.sent_packets);
            let lost = path.lost_packets.saturating_sub(window.lost_packets);
            window.sent_packets = path.sent_packets;
            window.lost_packets = path.lost_packets;
            lost > 0 && lost as f64 / sent.max(lost) as f64 > 0.02
        });
        let stalled = window
            .started
            .is_some_and(|t| t.elapsed() >= Duration::from_millis(100));
        let sample = if stalled || window.slow || loss {
            Some(false)
        } else {
            window
                .last_completed
                .filter(|(at, _)| at.elapsed() <= Duration::from_millis(1500))
                .map(|(_, healthy)| healthy)
        };
        window.slow = false;
        sample
    }
}

fn send_budget(bitrate_kbps: u32, base_rtt: Duration) -> u64 {
    let in_flight = f64::from(bitrate_kbps) * 125. * base_rtt.as_secs_f64().clamp(0.001, 1.);
    (in_flight as u64 + 32 * 1024).clamp(64 * 1024, 2 * 1024 * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn idle_refreshes_do_not_reuse_a_busy_workload_or_ratchet_quality_down() {
        let health = DeliveryHealth::default();
        let budget = Duration::from_millis(16);
        let t = Instant::now();
        for i in 0..100 {
            health.encoded_at(
                Duration::from_millis(30),
                budget,
                t + Duration::from_millis(i * 16),
            );
        }
        assert_eq!(health.sample(), Some(false));
        for second in 2..30 {
            health.begin_write();
            health.end_write();
            health.encoded_at(
                Duration::from_millis(100),
                budget,
                t + Duration::from_secs(second),
            );
            assert_eq!(health.sample(), Some(true));
        }
    }
    #[test]
    fn send_budget_shrinks_with_quality_without_capping_fast_wan_to_lan_bursts() {
        assert_eq!(send_budget(1000, Duration::from_millis(150)), 64 * 1024);
        assert_eq!(send_budget(48000, Duration::from_millis(1)), 64 * 1024);
        assert!(send_budget(24000, Duration::from_millis(80)) >= 240_000);
        assert_eq!(
            send_budget(u32::MAX, Duration::from_secs(10)),
            2 * 1024 * 1024
        );
    }
    #[test]
    fn sustained_frame_work_pressure_is_detected_but_one_spike_is_not() {
        let health = DeliveryHealth::default();
        let budget = Duration::from_millis(8);
        health.begin_write();
        health.end_write();
        for _ in 0..100 {
            health.encoded(Duration::from_millis(3), budget);
        }
        health.encoded(Duration::from_millis(50), budget);
        assert_eq!(health.sample(), Some(true));
        for _ in 0..100 {
            health.encoded(Duration::from_millis(14), budget);
        }
        assert_eq!(health.sample(), Some(false));
        for _ in 0..100 {
            health.encoded(Duration::from_millis(3), budget);
        }
        health.sample();
        assert_eq!(health.sample(), Some(true));
    }
    #[test]
    fn idle_stalls_and_recovery_require_fresh_writes() {
        let health = DeliveryHealth::default();
        assert_eq!(health.sample(), None);
        health.begin_write();
        health.end_write();
        assert_eq!(health.sample(), Some(true));
        health
            .window
            .lock()
            .unwrap()
            .last_completed
            .as_mut()
            .unwrap()
            .0 -= Duration::from_secs(2);
        assert_eq!(health.sample(), None);
        health.begin_write();
        *health.window.lock().unwrap().started.as_mut().unwrap() -= Duration::from_secs(1);
        assert_eq!(health.sample(), Some(false));
        health.end_write();
        assert_eq!(health.sample(), Some(false));
        health.begin_write();
        health.end_write();
        assert_eq!(health.sample(), Some(true));
    }
}
