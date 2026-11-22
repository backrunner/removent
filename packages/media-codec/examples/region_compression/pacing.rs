//! Offline, video-first serialization model. Capacity is for application media
//! bytes after reserving other traffic. It is not an estimate of link bandwidth.
//! Frame timestamps and the next video deadline share a monotonic microsecond clock.
const MILLIBITS_PER_BYTE: u128 = 8_000;
const DEADLINE_RESERVE_US: u128 = 1_000;
const MAX_PATCH_SERIALIZATION_US: u128 = 8_000;

pub struct PatchPacer {
    effective_kbps: u32,
    blocked: bool,
    last_us: Option<i64>,
    // One kbps serves one millibit per microsecond. Keep fractional bytes
    // between observations so that frequent feedback doesn't change the model.
    queued_millibits: u128,
}

impl PatchPacer {
    pub fn new(effective_kbps: u32) -> Self {
        Self {
            effective_kbps,
            blocked: false,
            last_us: None,
            queued_millibits: 0,
        }
    }

    fn advance(&mut self, at_us: i64) -> bool {
        if let Some(last_us) = self.last_us {
            if at_us < last_us {
                return false;
            }
            if !self.blocked {
                let served = u128::from(at_us.abs_diff(last_us)) * u128::from(self.effective_kbps);
                self.queued_millibits = self.queued_millibits.saturating_sub(served);
            }
        }
        self.last_us = Some(at_us);
        true
    }

    /// Set observed service conditions from this timestamp forward. A blocked
    /// or zero-rate link retains all debt. Ignore out-of-order feedback.
    pub fn set_link(&mut self, at_us: i64, effective_kbps: u32, blocked: bool) {
        if self.advance(at_us) {
            self.effective_kbps = effective_kbps;
            self.blocked = blocked;
        }
    }

    /// Once per source tick, charge this tick's video (including framing) and
    /// return the remaining budget for the *entire* optional patch batch.
    /// Enqueue video first. A previous tick still in flight precludes an
    /// optional batch. An idle link doesn't accumulate credit for a later burst.
    pub fn start_frame(&mut self, pts_us: i64, next_pts_us: i64, video_bytes: usize) -> usize {
        let in_order = self.advance(pts_us);
        let carried_debt = self.queued_millibits > 0;
        self.charge(video_bytes);
        if !in_order || self.blocked || carried_debt || next_pts_us <= pts_us {
            return 0;
        }
        let horizon = u128::from(next_pts_us.abs_diff(pts_us)).saturating_sub(DEADLINE_RESERVE_US);
        let rate = u128::from(self.effective_kbps);
        let spare = (horizon * rate).saturating_sub(self.queued_millibits);
        let usable = spare.min(MAX_PATCH_SERIALIZATION_US * rate);
        // The burst limit also bounds this conversion on 32-bit targets.
        (usable / MILLIBITS_PER_BYTE) as usize
    }

    /// Charge actual serialized bytes, including mandatory invalidations even
    /// if they alone exceed the offer. Call after constructing each batch.
    pub fn charge_patch(&mut self, actual_batch_bytes: usize) {
        self.charge(actual_batch_bytes);
    }

    fn charge(&mut self, bytes: usize) {
        self.queued_millibits = self
            .queued_millibits
            .saturating_add(bytes as u128 * MILLIBITS_PER_BYTE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_and_reserve_consume_deadline_capacity_including_partial_bytes() {
        let mut pacer = PatchPacer::new(2_000);
        // At 250 B/ms the 9 ms usable interval carries 2250 B in total.
        assert_eq!(pacer.start_frame(0, 10_003, 1_001), 1_249);
        pacer.charge_patch(1_249);
        assert_eq!(pacer.start_frame(10_003, 20_003, 2_250), 0);
        assert_eq!(pacer.start_frame(20_003, 20_500, 0), 0);
    }

    #[test]
    fn a_long_static_interval_never_builds_credit_or_starves_next_video() {
        let mut pacer = PatchPacer::new(2_000);
        assert_eq!(pacer.start_frame(0, 33_333, 8_200), 0);
        assert_eq!(pacer.start_frame(10_000_000, 20_000_000, 0), 2_000);
        pacer.charge_patch(2_000);
        assert_eq!(pacer.start_frame(10_004_000, 10_037_333, 200), 0);
        assert_eq!(pacer.start_frame(10_037_333, 10_070_666, 200), 2_000);
    }

    #[test]
    fn mandatory_overspend_and_blocking_suppress_future_optional_work() {
        let mut pacer = PatchPacer::new(1_000);
        assert_eq!(pacer.start_frame(0, 10_000, 1_125), 0);
        pacer.charge_patch(2_000);
        pacer.set_link(8_000, 1_000, true); // 2125 B remain in flight.
        assert_eq!(pacer.start_frame(20_000, 40_000, 0), 0);
        pacer.set_link(30_000, 2_000, false);
        assert_eq!(pacer.start_frame(38_496, 50_000, 0), 0); // one byte remains.
        assert_eq!(pacer.start_frame(38_500, 50_000, 0), 2_000);
    }

    #[test]
    fn fractional_service_survives_feedback_and_stale_feedback_is_ignored() {
        let mut pacer = PatchPacer::new(1);
        assert_eq!(pacer.start_frame(-10_000, 0, 1), 0);
        pacer.set_link(-6_000, 1, false); // half a byte drained.
        pacer.set_link(-7_000, 0, true); // stale.
        assert_eq!(pacer.start_frame(-2_000, 8_000, 0), 1);
        pacer.set_link(0, 0, false);
        assert_eq!(pacer.start_frame(10_000, 20_000, 1), 0);
        pacer.set_link(20_000, 2_000, false);
        assert_eq!(pacer.start_frame(19_999, 30_000, 1), 0); // stale video still charged.
        assert_eq!(pacer.start_frame(20_007, 30_007, 0), 0);
        assert_eq!(pacer.start_frame(20_008, 30_008, 0), 2_000);
        assert_eq!(pacer.start_frame(21_000, 20_000, 0), 0);
    }
}
