//! Capture readiness is not frame rate: an idle desktop may send no new pixels.
use std::time::{Duration, Instant};

pub struct CaptureHealth {
    started: Instant,
    has_frame: bool,
    unavailable_since: Option<Instant>,
    last_poll_frame: Option<Instant>,
}

impl Default for CaptureHealth {
    fn default() -> Self {
        Self::new(Instant::now())
    }
}

impl CaptureHealth {
    pub fn new(now: Instant) -> Self {
        Self {
            started: now,
            has_frame: false,
            unavailable_since: None,
            last_poll_frame: None,
        }
    }

    #[cfg(any(not(target_vendor = "apple"), test))]
    pub(crate) fn polling(now: Instant) -> Self {
        Self {
            last_poll_frame: Some(now),
            ..Self::new(now)
        }
    }

    /// Returns true for the first usable frame of this capture instance.
    pub fn frame(&mut self) -> bool {
        let first = !self.has_frame;
        self.has_frame = true;
        self.unavailable_since = None;
        if let Some(last) = &mut self.last_poll_frame {
            *last = Instant::now();
        }
        first
    }
    pub fn unavailable(&mut self, now: Instant) {
        self.unavailable_since.get_or_insert(now);
    }
    pub fn failure(&self, now: Instant) -> Option<&'static str> {
        if !self.has_frame && now.duration_since(self.started) >= Duration::from_secs(5) {
            Some("capture produced no first frame within five seconds")
        } else if self
            .unavailable_since
            .is_some_and(|t| now.duration_since(t) >= Duration::from_secs(2))
        {
            Some("capture remains blank or suspended")
        } else if self
            .last_poll_frame
            .is_some_and(|t| now.duration_since(t) >= Duration::from_secs(5))
        {
            Some("desktop capture worker stopped producing frames")
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn static_desktop_is_healthy_but_missing_first_frame_is_not() {
        let now = Instant::now();
        let mut health = CaptureHealth::new(now);
        assert!(health.failure(now + Duration::from_secs(5)).is_some());
        assert!(health.frame());
        assert!(!health.frame());
        assert!(health.failure(now + Duration::from_secs(3600)).is_none());
    }
    #[test]
    fn sustained_blank_times_out_and_valid_frame_recovers() {
        let now = Instant::now();
        let mut health = CaptureHealth::new(now);
        health.frame();
        health.unavailable(now);
        health.unavailable(now + Duration::from_secs(1));
        assert!(health.failure(now + Duration::from_secs(1)).is_none());
        assert!(health.failure(now + Duration::from_secs(2)).is_some());
        health.frame();
        assert!(health.failure(now + Duration::from_secs(10)).is_none());
    }

    #[test]
    fn polling_capture_detects_a_stall_after_its_first_frame() {
        let mut health = CaptureHealth::polling(Instant::now());
        health.frame();
        let last = health.last_poll_frame.unwrap();
        assert!(health.failure(last + Duration::from_secs(4)).is_none());
        assert_eq!(
            health.failure(last + Duration::from_secs(5)),
            Some("desktop capture worker stopped producing frames")
        );
    }
}
