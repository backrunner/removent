//! Actual presentation telemetry. Call only from the platform's presentation
//! completion callback; decode, UI receipt and paint are not presentation.
use std::sync::Mutex;

#[derive(Debug, Default)]
pub struct PresentationStats {
    pub frames: u64,
    pub repeated: u64,
    pub gap_p95_ms: f64,
    pub gap_p99_ms: f64,
    pub gap_max_ms: f64,
}

#[derive(Default)]
struct Window {
    previous_pts: Option<i64>,
    previous_time: Option<f64>,
    frames: u64,
    repeated: u64,
    gaps: Vec<f64>,
}

#[derive(Default)]
pub struct PresentationTelemetry(Mutex<Window>);

impl PresentationTelemetry {
    pub fn presented(&self, pts: i64, seconds: f64) {
        if !seconds.is_finite() || seconds <= 0. {
            return;
        }
        let mut w = self.0.lock().unwrap();
        if w.previous_pts == Some(pts) {
            w.repeated += 1;
            return;
        }
        if w.previous_time.is_some_and(|at| seconds <= at) {
            return;
        }
        if let Some(previous) = w.previous_time {
            if w.gaps.len() == 1024 {
                w.gaps.remove(0);
            }
            w.gaps.push((seconds - previous) * 1000.);
        }
        w.previous_pts = Some(pts);
        w.previous_time = Some(seconds);
        w.frames += 1;
    }

    pub fn sample(&self) -> PresentationStats {
        let mut w = self.0.lock().unwrap();
        w.gaps.sort_by(f64::total_cmp);
        let percentile = |p: usize| {
            w.gaps
                .get((w.gaps.len() * p).div_ceil(100).saturating_sub(1))
                .copied()
                .unwrap_or(0.)
        };
        let result = PresentationStats {
            frames: w.frames,
            repeated: w.repeated,
            gap_p95_ms: percentile(95),
            gap_p99_ms: percentile(99),
            gap_max_ms: percentile(100),
        };
        w.frames = 0;
        w.repeated = 0;
        w.gaps.clear();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_stall_in_a_sixty_frame_window_is_visible_at_p99() {
        let p = PresentationTelemetry::default();
        for i in 0..60 {
            p.presented(i, 1. + i as f64 / 60. + if i == 59 { 0.1 } else { 0. });
        }
        let s = p.sample();
        assert!(s.gap_p95_ms < 17.);
        assert!(s.gap_p99_ms > 100.);
    }
    #[test]
    fn redraw_is_not_a_new_video_frame_and_stalls_remain_visible() {
        let p = PresentationTelemetry::default();
        p.presented(1, 1.);
        p.presented(1, 1.016);
        p.presented(2, 1.1);
        p.presented(3, f64::NAN);
        p.presented(3, 0.9);
        let stats = p.sample();
        assert_eq!(stats.frames, 2);
        assert_eq!(stats.repeated, 1);
        assert!((stats.gap_max_ms - 100.).abs() < 0.001);
        assert_eq!(p.sample().frames, 0);
    }
}
