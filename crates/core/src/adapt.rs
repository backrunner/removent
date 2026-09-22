//! Readability-first adaptive quality. Preserve negotiated spatial detail, spend
//! fewer frames on a constrained link, and only recover after sustained evidence.
//! The per-frame budget is a conservative rate-control floor, not a proof that
//! arbitrary content/font sizes remain legible (see the codec quantizer ceiling).

use crate::QualityPreset;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QualityState {
    pub bitrate_kbps: u32,
    pub fps: u8,
    pub scale: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct Sample {
    pub rtt_ms: f32,
    pub loss_pct: f32,
    pub recv_kbps: u32,
    pub jitter_ms: f32,
    /// Submission to decoded-output latency, including decoder queueing.
    pub decode_ms: f32,
}

impl Sample {
    pub fn is_valid(&self) -> bool {
        [self.rtt_ms, self.loss_pct, self.jitter_ms, self.decode_ms]
            .iter()
            .all(|v| v.is_finite() && *v >= 0.)
            && self.loss_pct <= 100.
    }
}

const MIN_CHANGE_INTERVAL_MS: u64 = 2000;
const HEALTHY_UPGRADE_MS: u64 = 5000;
pub const MIN_REFRESH_FPS: u8 = 2;

/// 0.08 bits/pixel/frame, rounded up. An engineering guardrail for screen
/// content, complemented by a codec quantizer ceiling where supported.
pub fn readable_frame_bits(width: u32, height: u32) -> u64 {
    (u64::from(width.max(1)) * u64::from(height.max(1))).div_ceil(100) * 8
}

pub struct AdaptationController {
    state: QualityState,
    max_bitrate_kbps: u32,
    negotiated_fps: u8,
    base_fps: u8,
    min_fps: u8,
    frame_bits: u64,
    preferred_frame_bits: u64,
    healthy_since_ms: Option<u64>,
    last_change_ms: u64,
    now_ms: u64,
}

impl AdaptationController {
    pub fn new(bitrate_kbps: u32, fps: u8, preset: QualityPreset) -> Self {
        Self::with_dimensions(bitrate_kbps, fps, preset, 1920, 1080)
    }

    pub fn with_dimensions(
        bitrate_kbps: u32,
        fps: u8,
        preset: QualityPreset,
        width: u32,
        height: u32,
    ) -> Self {
        let cap = match preset {
            QualityPreset::Smooth => 4_000,
            QualityPreset::Balanced => 12_000,
            QualityPreset::HighQuality => 24_000,
            QualityPreset::Extreme => 48_000,
            QualityPreset::Auto => u32::MAX,
        };
        let max_bitrate_kbps = bitrate_kbps.clamp(1, cap);
        let frame_bits = readable_frame_bits(width, height);
        // A low manual bitrate may not afford the requested frame rate even
        // before congestion. Never start below the per-frame budget if avoidable.
        let base_fps = fps
            .max(1)
            .min((u64::from(max_bitrate_kbps) * 1000 / frame_bits).clamp(1, 255) as u8);
        Self {
            state: QualityState {
                bitrate_kbps: max_bitrate_kbps,
                fps: base_fps,
                scale: 1.0,
            },
            max_bitrate_kbps,
            negotiated_fps: fps.max(1),
            base_fps,
            min_fps: MIN_REFRESH_FPS.min(base_fps),
            frame_bits,
            preferred_frame_bits: (u64::from(max_bitrate_kbps) * 1000 / u64::from(base_fps))
                .max(frame_bits),
            healthy_since_ms: None,
            last_change_ms: 0,
            now_ms: 0,
        }
    }

    pub fn state(&self) -> QualityState {
        self.state
    }

    /// Capture geometry can change when a monitor mode changes. Refresh the
    /// readability budget before consuming the next delivery sample.
    pub fn set_dimensions(&mut self, width: u32, height: u32) {
        let bits = readable_frame_bits(width, height);
        if self.frame_bits == bits {
            return;
        }
        self.frame_bits = bits;
        self.base_fps = self
            .negotiated_fps
            .min((u64::from(self.max_bitrate_kbps) * 1000 / bits).clamp(1, 255) as u8);
        self.min_fps = MIN_REFRESH_FPS.min(self.base_fps);
        self.preferred_frame_bits =
            (u64::from(self.max_bitrate_kbps) * 1000 / u64::from(self.base_fps)).max(bits);
        self.state.bitrate_kbps = self.state.bitrate_kbps.max(self.bitrate_floor_kbps());
        self.state.fps = self
            .state
            .fps
            .min(self.base_fps)
            .min((u64::from(self.state.bitrate_kbps) * 1000 / bits).clamp(1, 255) as u8)
            .max(self.min_fps);
        self.pause_recovery();
    }

    pub fn bitrate_floor_kbps(&self) -> u32 {
        (self.frame_bits * u64::from(self.min_fps))
            .div_ceil(1000)
            .max(64)
            .min(u64::from(self.max_bitrate_kbps)) as u32
    }

    /// No fresh delivery evidence: idle time must not count toward recovery.
    pub fn pause_recovery(&mut self) {
        self.healthy_since_ms = None;
    }

    pub fn on_sample(&mut self, sample: &Sample, window_ms: u64) -> Option<QualityState> {
        self.now_ms = self.now_ms.saturating_add(window_ms);
        if !sample.is_valid() {
            self.pause_recovery();
            return None;
        }
        // A scheduler pause is not several seconds of healthy observations.
        if window_ms > 1000 {
            self.pause_recovery();
        }
        // Absolute RTT is not congestion: an otherwise healthy distant path
        // should retain quality. A slow decoder is receiver-side pressure too.
        let bad = sample.loss_pct > 2.0
            || sample.jitter_ms > 20.0
            || sample.decode_ms > (1000.0 / f32::from(self.state.fps)).max(40.0);
        if !bad && sample.loss_pct < 0.5 && sample.jitter_ms < 10. {
            self.healthy_since_ms.get_or_insert(self.now_ms);
        } else {
            self.pause_recovery();
        }
        if self.now_ms.saturating_sub(self.last_change_ms) < MIN_CHANGE_INTERVAL_MS {
            return None;
        }
        let before = self.state;
        if bad {
            let mut bitrate = (u64::from(self.state.bitrate_kbps) * 7 / 10) as u32;
            let mut fps = self.state.fps;
            if fps > 15 {
                // Keep bits/frame while first shedding motion smoothness.
                fps = ((u16::from(fps) * 7 / 10) as u8).max(15);
                bitrate = (u64::from(self.state.bitrate_kbps) * u64::from(fps)
                    / u64::from(self.state.fps)) as u32;
            } else if sample.recv_kbps > 0 {
                // Goodput is meaningful only WITH pressure. An idle screen's
                // tiny byte rate must never be mistaken for a slow link.
                let observed = (u64::from(sample.recv_kbps) * 85 / 100) as u32;
                bitrate = bitrate.min(observed.max(self.state.bitrate_kbps / 2));
            }
            self.state.bitrate_kbps = bitrate.max(self.bitrate_floor_kbps());
            self.state.fps = fps
                .min(
                    (u64::from(self.state.bitrate_kbps) * 1000 / self.frame_bits).clamp(1, 255)
                        as u8,
                )
                .max(self.min_fps);
        } else if self
            .healthy_since_ms
            .is_some_and(|t| self.now_ms.saturating_sub(t) >= HEALTHY_UPGRADE_MS)
        {
            self.state.bitrate_kbps = ((u64::from(self.state.bitrate_kbps) * 5)
                .div_ceil(4)
                .min(u64::from(self.max_bitrate_kbps)))
                as u32;
            // Restore detail budget before increasing motion smoothness.
            let affordable = (u64::from(self.state.bitrate_kbps) * 1000 / self.preferred_frame_bits)
                .clamp(1, u64::from(self.base_fps)) as u8;
            self.state.fps = self.state.fps.max(affordable);
            self.healthy_since_ms = Some(self.now_ms);
        }
        if self.state != before {
            self.last_change_ms = self.now_ms;
            Some(self.state)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn good() -> Sample {
        Sample {
            rtt_ms: 180.,
            loss_pct: 0.,
            recv_kbps: 0,
            jitter_ms: 1.,
            decode_ms: 1.,
        }
    }
    fn bad() -> Sample {
        Sample {
            loss_pct: 5.,
            recv_kbps: 2000,
            ..good()
        }
    }
    fn feed(c: &mut AdaptationController, s: Sample, seconds: u64) {
        for _ in 0..seconds * 4 {
            c.on_sample(&s, 250);
        }
    }

    #[test]
    fn first_downgrade_preserves_bits_per_frame_and_resolution() {
        let mut c = AdaptationController::new(20_000, 60, QualityPreset::Auto);
        feed(&mut c, bad(), 2);
        assert_eq!(
            c.state(),
            QualityState {
                bitrate_kbps: 14_000,
                fps: 42,
                scale: 1.
            }
        );
        let before = c.state();
        feed(&mut c, bad(), 1);
        assert_eq!(c.state(), before, "two-second change interval");
    }

    #[test]
    fn persistent_loss_stops_at_dimension_aware_readability_floor() {
        for (w, h) in [(1920, 1080), (1280, 720), (320, 240)] {
            let mut c =
                AdaptationController::with_dimensions(20_000, 60, QualityPreset::Auto, w, h);
            for _ in 0..600 {
                c.on_sample(&bad(), 250);
                let s = c.state();
                assert_eq!(s.scale, 1.0);
                assert!(
                    u64::from(s.bitrate_kbps) * 1000 / u64::from(s.fps)
                        >= readable_frame_bits(w, h)
                );
            }
            assert_eq!(c.state().bitrate_kbps, c.bitrate_floor_kbps());
            assert!(c.state().fps >= MIN_REFRESH_FPS);
        }
        assert_eq!(
            AdaptationController::new(8000, 30, QualityPreset::Auto).bitrate_floor_kbps(),
            332
        );
    }

    #[test]
    fn recovery_restores_detail_before_fps_and_never_exceeds_ceiling() {
        let mut c = AdaptationController::new(8000, 30, QualityPreset::Auto);
        let initial = c.state();
        feed(&mut c, bad(), 120);
        let floor = c.state();
        feed(&mut c, good(), 5);
        assert_eq!(c.state(), floor);
        feed(&mut c, good(), 1);
        assert!(c.state().bitrate_kbps > floor.bitrate_kbps);
        assert_eq!(c.state().fps, floor.fps);
        feed(&mut c, good(), 120);
        assert_eq!(c.state(), initial);
    }

    #[test]
    fn static_screen_or_high_rtt_alone_never_downgrades() {
        let mut c = AdaptationController::new(8000, 30, QualityPreset::Auto);
        let initial = c.state();
        feed(&mut c, good(), 120);
        assert_eq!(c.state(), initial);
    }

    #[test]
    fn decode_pressure_also_reduces_frame_rate() {
        let mut c = AdaptationController::new(8000, 30, QualityPreset::Auto);
        feed(
            &mut c,
            Sample {
                decode_ms: 90.,
                ..good()
            },
            2,
        );
        assert!(c.state().fps < 30);
    }

    #[test]
    fn monitor_changes_recompute_the_readability_budget() {
        let mut c = AdaptationController::with_dimensions(8000, 60, QualityPreset::Auto, 320, 240);
        feed(&mut c, bad(), 120);
        c.set_dimensions(1920, 1080);
        assert_eq!(
            c.state(),
            QualityState {
                bitrate_kbps: 332,
                fps: 2,
                scale: 1.
            }
        );
        feed(&mut c, good(), 120);
        assert_eq!(c.state().fps, 48);
        c.set_dimensions(320, 240);
        feed(&mut c, good(), 10);
        assert_eq!(c.state().fps, 60);
    }

    #[test]
    fn missing_invalid_or_stale_evidence_cannot_finish_recovery() {
        let mut c = AdaptationController::new(8000, 30, QualityPreset::Auto);
        feed(&mut c, bad(), 20);
        let before = c.state();
        feed(&mut c, good(), 4);
        c.pause_recovery();
        feed(&mut c, good(), 4);
        c.on_sample(
            &Sample {
                jitter_ms: f32::NAN,
                ..good()
            },
            250,
        );
        feed(&mut c, good(), 4);
        c.on_sample(&good(), 30_000);
        feed(&mut c, good(), 4);
        assert_eq!(c.state(), before);
    }

    #[test]
    fn manual_caps_and_low_bitrate_are_respected() {
        for preset in [
            QualityPreset::Smooth,
            QualityPreset::Balanced,
            QualityPreset::HighQuality,
            QualityPreset::Extreme,
        ] {
            let mut c = AdaptationController::new(2000, 30, preset);
            assert_eq!(c.state().fps, 12);
            feed(&mut c, bad(), 60);
            feed(&mut c, good(), 120);
            assert_eq!(c.state().bitrate_kbps, 2000);
            assert_eq!(c.state().fps, 12);
        }
        let mut c = AdaptationController::new(10, 1, QualityPreset::Auto);
        feed(&mut c, bad(), 60);
        assert_eq!(
            c.state(),
            QualityState {
                bitrate_kbps: 10,
                fps: 1,
                scale: 1.
            }
        );
    }
}
