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
}

impl DeliveryHealth {
    pub fn new(conn: RvpConnection) -> Self {
        let path = conn.inner().stats().path;
        Self {
            window: Mutex::new(Window {
                sent_packets: path.sent_packets,
                lost_packets: path.lost_packets,
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

#[cfg(test)]
mod tests {
    use super::*;
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
