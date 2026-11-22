//! Diagnostic counts of ScreenCaptureKit redraws, measured before frame coalescing.
//! SCK reports the union of moved and redrawn rectangles in surface pixels.
use screencapturekit::cg::CGRect;
use std::time::{Duration, Instant};

const TILE: usize = 64;
const INTERVAL: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Grid {
    width: usize,
    height: usize,
    columns: usize,
    touched_rows: Vec<u128>,
    hits: Vec<u32>,
}

impl Grid {
    fn new(width: usize, height: usize) -> Self {
        // One row mask covers 8192 pixels. Unsupported geometry still appears
        // in the log as unknown and does not affect the capture itself.
        let columns = width.div_ceil(TILE);
        let rows = height.div_ceil(TILE);
        if columns == 0 || rows == 0 || columns > 128 || rows > 128 {
            return Self {
                width,
                height,
                ..Self::default()
            };
        }
        Self {
            width,
            height,
            columns,
            touched_rows: vec![0; rows],
            hits: vec![0; columns * rows],
        }
    }

    /// Commit one hit per touched tile per sample, even for overlapping rects.
    /// Reject ambiguous or invalid metadata for the whole sample.
    fn observe(&mut self, rects: &[CGRect]) -> bool {
        if self.columns == 0 || rects.is_empty() {
            return false;
        }
        self.touched_rows.fill(0);
        for rect in rects {
            let (left, top) = (rect.min_x(), rect.min_y());
            let (right, bottom) = (rect.max_x(), rect.max_y());
            if !(left >= 0.0
                && top >= 0.0
                && right > left
                && bottom > top
                && right <= self.width as f64
                && bottom <= self.height as f64)
            {
                return false;
            }
            let x0 = left as usize / TILE;
            let y0 = top as usize / TILE;
            let x1 = (right.ceil() as usize).div_ceil(TILE);
            let y1 = (bottom.ceil() as usize).div_ceil(TILE);
            let bits = (u128::MAX >> (128 - (x1 - x0))) << x0;
            for row in &mut self.touched_rows[y0..y1] {
                *row |= bits;
            }
        }
        for (row_index, &row) in self.touched_rows.iter().enumerate() {
            let offset = row_index * self.columns;
            let mut bits = row;
            while bits != 0 {
                self.hits[offset + bits.trailing_zeros() as usize] += 1;
                bits &= bits - 1;
            }
        }
        true
    }
}

pub(super) struct RegionDiagnostics {
    generation: u64,
    started: Instant,
    grid: Grid,
    measured: u32,
    unknown: u32,
    idle: u32,
    analysis_time: Duration,
    peak_analysis_time: Duration,
}

impl RegionDiagnostics {
    pub(super) fn from_env(generation: u64) -> Option<Self> {
        (std::env::var_os("REMOVENT_REGION_DIAGNOSTICS").as_deref()
            == Some(std::ffi::OsStr::new("1")))
        .then(|| Self::new(generation))
    }

    fn new(generation: u64) -> Self {
        Self {
            generation,
            started: Instant::now(),
            grid: Grid::default(),
            measured: 0,
            unknown: 0,
            idle: 0,
            analysis_time: Duration::ZERO,
            peak_analysis_time: Duration::ZERO,
        }
    }

    fn advance(&mut self, now: Instant, resized: bool) {
        if resized || now.duration_since(self.started) >= INTERVAL {
            self.report(now);
            self.started = now;
            self.grid.hits.fill(0);
            self.measured = 0;
            self.unknown = 0;
            self.idle = 0;
            self.analysis_time = Duration::ZERO;
            self.peak_analysis_time = Duration::ZERO;
        }
    }

    pub(super) fn frame(
        &mut self,
        width: usize,
        height: usize,
        dirty_rects: impl FnOnce() -> Option<Vec<CGRect>>,
    ) {
        let resized = (self.grid.width, self.grid.height) != (width, height);
        self.advance(Instant::now(), resized);
        let started = Instant::now();
        if resized {
            self.grid = Grid::new(width, height);
        }
        // The bridge maps both missing attachments and empty arrays to None.
        // Only an explicit SCFrameStatus::Idle is evidence of a static sample.
        if dirty_rects().is_some_and(|rects| self.grid.observe(&rects)) {
            self.measured += 1;
        } else {
            self.unknown += 1;
        }
        let elapsed = started.elapsed();
        self.analysis_time += elapsed;
        self.peak_analysis_time = self.peak_analysis_time.max(elapsed);
    }

    pub(super) fn idle(&mut self) {
        self.advance(Instant::now(), false);
        self.idle += 1;
    }

    fn report(&self, now: Instant) {
        if self.measured == 0 && self.unknown == 0 && self.idle == 0 {
            return;
        }
        // An idle desktop may give us no callback at the deadline. Its next
        // sample starts a new window; the silent gap must not dilute this one.
        let duration = now.duration_since(self.started).min(INTERVAL);
        let seconds = duration.as_secs_f64();
        let mut touched_tiles = 0;
        let mut frequent_tiles = 0;
        let mut tile_updates = 0u64;
        let mut peak_hits = 0;
        let mut peak_tile = None;
        for (index, &hits) in self.grid.hits.iter().enumerate().filter(|(_, h)| **h > 0) {
            touched_tiles += 1;
            tile_updates += u64::from(hits);
            if f64::from(hits) >= 4.0 * seconds {
                frequent_tiles += 1;
            }
            if hits > peak_hits {
                peak_hits = hits;
                peak_tile = Some((index % self.grid.columns, index / self.grid.columns));
            }
        }
        tracing::info!(
            generation = self.generation,
            width = self.grid.width,
            height = self.grid.height,
            seconds,
            measured_frames = self.measured,
            unknown_frames = self.unknown,
            idle_callbacks = self.idle,
            tile_size = TILE,
            tiles = self.grid.hits.len(),
            touched_tiles,
            // On immediate resize/stop, a short partial sample cannot establish frequency.
            tiles_at_least_4hz = (duration >= Duration::from_secs(1)).then_some(frequent_tiles),
            tile_updates,
            analysis_us = self.analysis_time.as_micros() as u64,
            peak_analysis_us = self.peak_analysis_time.as_micros() as u64,
            peak_hits,
            ?peak_tile,
            "capture region diagnostics"
        );
    }
}

impl Drop for RegionDiagnostics {
    fn drop(&mut self) {
        self.report(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_rects_and_exact_edges_count_once_per_sample() {
        let mut grid = Grid::new(131, 65);
        let a = CGRect::new(63.5, 0.0, 0.5, 64.0);
        let b = CGRect::new(63.5, 63.5, 1.0, 1.0);
        let c = CGRect::new(130.0, 64.0, 1.0, 1.0);
        assert!(grid.observe(&[a, a]));
        assert_eq!(grid.hits, [1, 0, 0, 0, 0, 0]);
        assert!(grid.observe(&[a, b, c]));
        assert_eq!(grid.hits, [2, 1, 0, 1, 1, 1]);
    }

    #[test]
    fn invalid_sample_never_partially_updates_counts() {
        let mut grid = Grid::new(8192, 64);
        let full = CGRect::new(0., 0., 8192., 64.);
        assert!(grid.observe(&[full]));
        assert!(grid.hits.iter().all(|h| *h == 1));
        for rect in [
            CGRect::new(0., 0., f64::NAN, 5.),
            CGRect::new(8192., 0., 1., 1.),
            CGRect::new(2., 0., -1., 1.),
        ] {
            assert!(!grid.observe(&[full, rect]));
        }
        assert!(grid.hits.iter().all(|h| *h == 1));
        assert!(!Grid::new(8193, 64).observe(&[full]));
    }

    #[test]
    fn missing_metadata_and_resize_do_not_contaminate_tile_frequency() {
        let mut state = RegionDiagnostics::new(1);
        let rect = CGRect::new(0., 0., 1., 1.);
        state.frame(128, 64, || Some(vec![rect]));
        state.frame(128, 64, || None);
        state.frame(128, 64, || Some(vec![]));
        state.idle();
        assert_eq!((state.measured, state.unknown, state.idle), (1, 2, 1));
        assert_eq!(state.grid.hits, [1, 0]);
        state.frame(64, 128, || Some(vec![CGRect::new(0., 64., 1., 1.)]));
        assert_eq!(state.grid.hits, [0, 1]);
        assert_eq!((state.measured, state.unknown, state.idle), (1, 0, 0));
    }

    #[tokio::test]
    async fn publish_records_coalesced_captures_and_skips_metadata_when_disabled() {
        use crate::sck::{AudioChunker, AudioState, SckOutput};
        use std::sync::{Arc, Mutex};

        let (video_tx, mut rx) = removent_core::latest::channel();
        let mut output = SckOutput {
            video_tx,
            generation: 42,
            health: Arc::default(),
            regions: Some(Mutex::new(RegionDiagnostics::new(42))),
            audio_tx: None,
            audio: Mutex::new(AudioState {
                chunker: AudioChunker::new(),
                dropped_frames: 0,
                last_drop_warn: None,
            }),
        };
        let frame = |pts| crate::CapturedVideoFrame::new(vec![0; 128 * 4], 128, 1, pts);
        for pts in 0..2 {
            output.publish_frame(frame(pts), || {
                Some(vec![CGRect::new(pts as f64 * 64., 0., 1., 1.)])
            });
        }
        assert_eq!(rx.recv().await.unwrap().pts_us, 1);
        let diagnostics = output.regions.take().unwrap().into_inner().unwrap();
        assert_eq!(diagnostics.grid.hits, [1, 1]);
        output.publish_frame(frame(2), || panic!("disabled diagnostics read metadata"));
        let delivered = rx.recv().await.unwrap();
        assert_eq!((delivered.pts_us, delivered.generation), (2, 42));
    }
}
