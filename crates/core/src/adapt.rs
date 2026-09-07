//! Adaptive quality controller (protocol.md §6.4).
//!
//! Input: StatsReport sample windows. Output: bitrate/fps/scale decisions.
//! Downgrade: loss>2% or jitter>20ms → bitrate×0.7 (floor 1000kbps);
//! on continued degradation, lower fps next, then scale. Upgrade ×1.25 after
//! 5s of sustained health. Hysteresis: minimum 2s between changes; non-Auto
//! manual presets act as an upper-bound clamp.

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
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Health {
    Degrading,
    Healthy,
}

const MIN_CHANGE_INTERVAL_MS: u64 = 2000;
const HEALTHY_UPGRADE_MS: u64 = 5000;
pub const BITRATE_FLOOR_KBPS: u32 = 1000;

pub struct AdaptationController {
    state: QualityState,
    max_bitrate_kbps: u32,
    base_fps: u8,
    scale_enabled: bool,
    health: Health,
    degraded_windows: u32,
    healthy_since_ms: Option<u64>,
    last_change_ms: u64,
    now_ms: u64,
}

impl AdaptationController {
    pub fn new(bitrate_kbps: u32, fps: u8, preset: QualityPreset) -> Self {
        let mut c = Self {
            state: QualityState {
                bitrate_kbps,
                fps,
                scale: 1.0,
            },
            max_bitrate_kbps: bitrate_kbps,
            base_fps: fps,
            scale_enabled: true,
            health: Health::Healthy,
            degraded_windows: 0,
            healthy_since_ms: None,
            last_change_ms: 0,
            now_ms: 0,
        };
        if preset != QualityPreset::Auto {
            let cap = match preset {
                QualityPreset::Smooth => 4_000,
                QualityPreset::Balanced => 12_000,
                QualityPreset::HighQuality => 24_000,
                QualityPreset::Extreme => 48_000,
                QualityPreset::Auto => unreachable!(),
            };
            c.state.bitrate_kbps = c.state.bitrate_kbps.min(cap);
            c.state.fps = c.state.fps.min(c.base_fps);
            c.max_bitrate_kbps = bitrate_kbps.min(cap);
        }
        c
    }

    pub fn state(&self) -> QualityState {
        self.state
    }

    /// Legacy clients cannot identify the geometry used by queued input events.
    pub fn set_scale_enabled(&mut self, enabled: bool) {
        self.scale_enabled = enabled;
    }

    /// No fresh delivery evidence: do not count idle time toward recovery.
    pub fn pause_recovery(&mut self) {
        self.healthy_since_ms = None;
    }

    fn is_bad(s: &Sample) -> bool {
        s.loss_pct > 2.0 || s.jitter_ms > 20.0
    }

    /// Feed one window sample; returns the new decision (if any).
    pub fn on_sample(&mut self, sample: &Sample, window_ms: u64) -> Option<QualityState> {
        self.now_ms += window_ms;
        let bad = Self::is_bad(sample);

        match bad {
            true => {
                self.health = Health::Degrading;
                self.degraded_windows = self.degraded_windows.saturating_add(1).min(3);
                self.healthy_since_ms = None;
            }
            false => {
                self.health = Health::Healthy;
                self.degraded_windows = 0;
                if self.healthy_since_ms.is_none() {
                    self.healthy_since_ms = Some(self.now_ms);
                }
            }
        }

        if self.now_ms.saturating_sub(self.last_change_ms) < MIN_CHANGE_INTERVAL_MS {
            return None;
        }

        match self.health {
            Health::Degrading if self.degraded_windows >= 1 => {
                let before = self.state;
                if self.state.bitrate_kbps > BITRATE_FLOOR_KBPS {
                    self.state.bitrate_kbps =
                        ((self.state.bitrate_kbps as f64 * 0.7) as u32).max(BITRATE_FLOOR_KBPS);
                } else if self.degraded_windows >= 2 && self.state.fps > 15 {
                    self.state.fps = (self.state.fps / 2).max(15);
                } else if self.degraded_windows >= 3 && self.scale_enabled {
                    self.state.scale = (self.state.scale - 0.25).max(0.5);
                }
                if self.state != before {
                    self.last_change_ms = self.now_ms;
                    Some(self.state)
                } else {
                    None
                }
            }
            Health::Healthy
                if self
                    .healthy_since_ms
                    .is_some_and(|t0| self.now_ms.saturating_sub(t0) >= HEALTHY_UPGRADE_MS)
                    && (self.state.bitrate_kbps < self.max_bitrate_kbps
                        || self.state.fps < self.base_fps
                        || self.state.scale < 1.0) =>
            {
                // Upgrade: recover bitrate toward the peak; also restore the fps/scale cut during downgrade.
                if self.state.scale < 1.0 {
                    self.state.scale = (self.state.scale + 0.25).min(1.0);
                } else if self.state.fps < self.base_fps {
                    self.state.fps = (self.state.fps.saturating_mul(2)).min(self.base_fps);
                } else {
                    self.state.bitrate_kbps = ((self.state.bitrate_kbps as f64 * 1.25).ceil()
                        as u32)
                        .min(self.max_bitrate_kbps);
                }
                self.healthy_since_ms = Some(self.now_ms);
                self.last_change_ms = self.now_ms;
                Some(self.state)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good() -> Sample {
        Sample {
            rtt_ms: 1.0,
            loss_pct: 0.0,
            recv_kbps: 10_000,
            jitter_ms: 1.0,
        }
    }
    fn bad() -> Sample {
        Sample {
            rtt_ms: 30.0,
            loss_pct: 5.0,
            recv_kbps: 4_000,
            jitter_ms: 40.0,
        }
    }

    #[test]
    fn idle_time_cannot_finish_recovery_and_legacy_scale_stays_fixed() {
        let mut c = AdaptationController::new(1000, 30, QualityPreset::Auto);
        c.set_scale_enabled(false);
        for _ in 0..30 {
            c.on_sample(&bad(), 2500);
        }
        assert_eq!(c.state().fps, 15);
        assert_eq!(c.state().scale, 1.0);
        for _ in 0..8 {
            c.on_sample(&good(), 250);
        }
        c.pause_recovery();
        for _ in 0..8 {
            assert!(c.on_sample(&good(), 250).is_none());
        }
        for _ in 0..16 {
            c.on_sample(&good(), 250);
        }
        assert_eq!(c.state().fps, 30);
    }

    #[test]
    fn sustained_health_recovers_after_a_long_outage() {
        let mut controller = AdaptationController::new(8000, 60, QualityPreset::Auto);
        for _ in 0..2400 {
            controller.on_sample(&bad(), 250);
        }
        let degraded = controller.state();
        for _ in 0..24 {
            controller.on_sample(&good(), 250);
        }
        assert!(controller.state().scale > degraded.scale);
        assert_eq!(controller.state().bitrate_kbps, degraded.bitrate_kbps);
        assert_eq!(controller.state().fps, degraded.fps);
    }

    #[test]
    fn healthy_samples_never_trigger_another_downgrade() {
        let mut controller = AdaptationController::new(20000, 60, QualityPreset::Auto);
        for _ in 0..4 {
            controller.on_sample(&bad(), 250);
        }
        for _ in 0..10 {
            let before = controller.state();
            controller.on_sample(&good(), 2500);
            assert!(controller.state().bitrate_kbps >= before.bitrate_kbps);
        }
    }

    #[test]
    fn manual_preset_does_not_exceed_negotiated_bitrate() {
        for preset in [
            QualityPreset::Smooth,
            QualityPreset::Balanced,
            QualityPreset::HighQuality,
            QualityPreset::Extreme,
        ] {
            let mut controller = AdaptationController::new(2000, 30, preset);
            for _ in 0..60 {
                controller.on_sample(&good(), 500);
            }
            assert_eq!(controller.state().bitrate_kbps, 2000);
        }
    }

    #[test]
    fn degrades_bitrate_on_loss() {
        let mut c = AdaptationController::new(20_000, 60, QualityPreset::Auto);
        assert!(c.on_sample(&bad(), 500).is_none());
        let d = c.on_sample(&bad(), 1600).expect("should degrade");
        assert!(d.bitrate_kbps < 20_000);
        assert!((d.bitrate_kbps as f64 - 14_000f64).abs() < 100.0);
        assert_eq!(d.fps, 60);
    }

    #[test]
    fn sustained_degradation_hits_floor_then_fps_then_scale() {
        let mut c = AdaptationController::new(1200, 30, QualityPreset::Auto);
        for _ in 0..12 {
            c.on_sample(&bad(), 2100);
        }
        let st = c.state();
        assert!(st.fps <= 15 || st.scale < 1.0, "{st:?}");
    }

    #[test]
    fn upgrades_after_sustained_health() {
        let mut c = AdaptationController::new(8_000, 60, QualityPreset::Auto);
        assert!(c.on_sample(&bad(), 2500).is_some());
        let degraded = c.state();
        let mut upgraded = None;
        let mut t = 2600u64;
        while t < 25_000 {
            if let Some(d) = c.on_sample(&good(), 500)
                && d.bitrate_kbps > degraded.bitrate_kbps
            {
                upgraded = Some(d);
                break;
            }
            t += 500;
        }
        let d = upgraded.expect("should upgrade");
        assert!(d.bitrate_kbps > degraded.bitrate_kbps);
    }

    #[test]
    fn upgrade_restores_fps_and_scale() {
        // Degrade until fps/scale are cut, then restore health: the upgrade must restore fps and scale.
        let mut c = AdaptationController::new(1200, 30, QualityPreset::Auto);
        for _ in 0..12 {
            c.on_sample(&bad(), 2100);
        }
        let degraded = c.state();
        assert!(degraded.fps < 30 || degraded.scale < 1.0, "{degraded:?}");
        for _ in 0..60 {
            c.on_sample(&good(), 500);
        }
        let st = c.state();
        assert_eq!(st.fps, 30, "fps should be restored, got {st:?}");
        assert_eq!(st.scale, 1.0, "scale should be restored, got {st:?}");
    }

    #[test]
    fn manual_preset_caps_upgrades() {
        let mut c = AdaptationController::new(4_000, 60, QualityPreset::Smooth);
        for _ in 0..60 {
            c.on_sample(&good(), 300);
        }
        assert!(c.state().bitrate_kbps <= 4_000);
    }

    #[test]
    fn single_good_sample_does_not_recover() {
        let mut c = AdaptationController::new(20_000, 60, QualityPreset::Auto);
        c.on_sample(&bad(), 2500);
        let after = c.state().bitrate_kbps;
        assert!(c.on_sample(&good(), 2200).is_none());
        assert_eq!(c.state().bitrate_kbps, after);
    }

    #[test]
    fn auto_keeps_constructor_bounds() {
        let c = AdaptationController::new(20_000, 60, QualityPreset::Auto);
        assert_eq!(c.state().bitrate_kbps, 20_000);
    }

    #[test]
    fn smooth_clamps_high_constructor() {
        let c = AdaptationController::new(40_000, 60, QualityPreset::Smooth);
        assert_eq!(c.state().bitrate_kbps, 4_000);
    }
}
