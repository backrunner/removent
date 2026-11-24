use removent_core::QualityState;
use serde_json::json;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::watch;

const LOW_KBPS: u64 = 2_000;

#[derive(Default)]
struct Timeline {
    reduced: Option<Instant>,
    restored: Option<Instant>,
    frames: Vec<(Instant, f64)>,
    quality: Vec<(Instant, u32)>,
}

pub struct Step {
    pub epoch: Instant,
    timeline: Mutex<Timeline>,
}

impl Step {
    pub fn new() -> Self {
        Self {
            epoch: Instant::now(),
            timeline: Mutex::default(),
        }
    }

    pub fn frame(&self, pts_us: u64) {
        let now = Instant::now();
        let source = self.epoch + Duration::from_micros(pts_us);
        self.timeline
            .lock()
            .unwrap()
            .frames
            .push((now, ms(now.saturating_duration_since(source))));
    }

    pub async fn sample(
        self: Arc<Self>,
        started: Instant,
        rate: Arc<AtomicU64>,
        mut quality: watch::Receiver<QualityState>,
    ) -> anyhow::Result<()> {
        let initial = rate.load(Ordering::Relaxed);
        for (offset, next) in [(4, LOW_KBPS), (16, initial)] {
            let due = tokio::time::Instant::from_std(started + Duration::from_secs(offset));
            loop {
                tokio::select! {
                    _ = tokio::time::sleep_until(due) => break,
                    change = quality.changed() => {
                        change?;
                        self.record(quality.borrow_and_update().bitrate_kbps);
                    }
                }
            }
            let mut timeline = self.timeline.lock().unwrap();
            rate.store(next, Ordering::Relaxed);
            let now = Instant::now();
            if next == LOW_KBPS {
                timeline.reduced = Some(now);
            } else {
                timeline.restored = Some(now);
            }
        }
        while quality.changed().await.is_ok() {
            self.record(quality.borrow_and_update().bitrate_kbps);
        }
        Ok(())
    }

    fn record(&self, bitrate_kbps: u32) {
        self.timeline
            .lock()
            .unwrap()
            .quality
            .push((Instant::now(), bitrate_kbps));
    }

    pub fn report(&self, carrier: &str, started: Instant, ended: Instant) -> anyhow::Result<()> {
        use anyhow::Context;
        let timeline = self.timeline.lock().unwrap();
        let reduced = timeline
            .reduced
            .context("missing downward bandwidth step")?;
        let restored = timeline.restored.context("missing upward bandwidth step")?;
        let first = |from: Instant, to: Instant, predicate: &dyn Fn(u32) -> bool| {
            timeline
                .quality
                .iter()
                .find(|&&(at, bitrate)| at >= from && at < to && predicate(bitrate))
                .map(|(at, _)| ms(at.duration_since(from)))
        };
        let age = |from: Instant, to: Instant| {
            let ages: Vec<_> = timeline
                .frames
                .iter()
                .filter(|&&(at, _)| at >= from && at < to)
                .map(|(_, age)| *age)
                .collect();
            json!({ "frames": ages.len(), "p95_ms": super::p(&ages, 95) })
        };
        let quality_before = |time| {
            timeline
                .quality
                .iter()
                .rev()
                .find(|&&(at, _)| at < time)
                .map_or(12_000, |(_, b)| *b)
        };
        let before_drop = quality_before(reduced);
        let mut previous = quality_before(restored);
        let first_recovery = timeline
            .quality
            .iter()
            .filter(|&&(at, _)| at >= restored && at < ended)
            .find(|&&(_, bitrate)| bitrate > std::mem::replace(&mut previous, bitrate))
            .map(|(at, _)| ms(at.duration_since(restored)));
        eprintln!(
            "{}",
            json!({
                "carrier": carrier,
                "drop_at_ms": ms(reduced.duration_since(started)),
                "restore_at_ms": ms(restored.duration_since(started)),
                "before_drop_kbps": before_drop,
                "first_cut_ms": first(reduced, restored, &|rate| rate < before_drop),
                "fits_low_link_ms": if u64::from(before_drop) <= LOW_KBPS { Some(0.) } else {
                    first(reduced, restored, &|rate| u64::from(rate) <= LOW_KBPS)
                },
                "first_recovery_ms": first_recovery,
                "age_before": age(started, reduced),
                "age_low": age(reduced, restored),
                "age_recovery": age(restored, ended),
                "quality": timeline.quality.iter().filter(|(at, _)| *at <= ended)
                    .map(|(at, b)| [at.saturating_duration_since(started).as_millis() as u64, u64::from(*b)])
                    .collect::<Vec<_>>(),
            })
        );
        Ok(())
    }
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.
}
