//! Device-clocked audio output shared by desktop and iOS.
//!
//! Producers may allocate; the renderer never locks or allocates. A short
//! prebuffer absorbs delivery jitter. Source timestamps detect discontinuities,
//! local source-time estimates expire stale blocks, and a bounded rate servo
//! accommodates independent capture/output clocks without growing the queue.

use crossbeam_queue::ArrayQueue;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

pub const RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;
pub const BLOCK_FRAMES: usize = 480;
pub const BLOCK_SAMPLES: usize = BLOCK_FRAMES * CHANNELS;
// Six-period capture batches can already be 50 ms old on arrival. Include the
// device prebuffer and one callback without mistaking that for a stale backlog.
pub const MAX_AUDIO_AGE: Duration = Duration::from_millis(80);
// One largest normal capture batch plus the two-period device reserve.
const QUEUE_BLOCKS: usize = 8;
const TARGET_FRAMES: usize = BLOCK_FRAMES * 2;
const FADE_FRAMES: usize = 240;

#[derive(Debug, Clone)]
pub struct DecodedAudio {
    pub samples: Vec<i16>,
    pub pts_us: i64,
    /// A local monotonic estimate, never a subtraction of remote/local epochs.
    pub source_at: Instant,
    pub discontinuity: bool,
}

impl std::ops::Deref for DecodedAudio {
    type Target = [i16];
    fn deref(&self) -> &Self::Target {
        &self.samples
    }
}

struct Block {
    samples: [i16; BLOCK_SAMPLES],
    frames: usize,
    pts_us: i64,
    source_at: Instant,
    discontinuity: bool,
}

#[derive(Default, Debug, Clone, Copy)]
pub struct AudioStats {
    pub queued_frames: usize,
    pub dropped_blocks: u64,
    pub underruns: u64,
    pub discontinuities: u64,
    pub rendered_frames: u64,
    pub callback_frames: u64,
    pub callback_rate: u64,
    pub device_delay_us: u64,
}

pub struct AudioQueue {
    blocks: ArrayQueue<Block>,
    epoch: AtomicU64,
    dropped: AtomicU64,
    underruns: AtomicU64,
    discontinuities: AtomicU64,
    rendered: AtomicU64,
    callback_frames: AtomicU64,
    callback_rate: AtomicU64,
    device_delay_us: AtomicU64,
}

impl Default for AudioQueue {
    fn default() -> Self {
        Self {
            blocks: ArrayQueue::new(QUEUE_BLOCKS),
            epoch: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
            discontinuities: AtomicU64::new(0),
            rendered: AtomicU64::new(0),
            callback_frames: AtomicU64::new(0),
            callback_rate: AtomicU64::new(0),
            device_delay_us: AtomicU64::new(0),
        }
    }
}

impl AudioQueue {
    pub fn push(&self, frame: DecodedAudio) {
        if frame.samples.is_empty()
            || frame.samples.len() > BLOCK_SAMPLES
            || !frame.samples.len().is_multiple_of(CHANNELS)
        {
            return;
        }
        let mut samples = [0; BLOCK_SAMPLES];
        samples[..frame.samples.len()].copy_from_slice(&frame.samples);
        let block = Block {
            samples,
            frames: frame.samples.len() / CHANNELS,
            pts_us: frame.pts_us,
            source_at: frame.source_at,
            discontinuity: frame.discontinuity,
        };
        if self.blocks.force_push(block).is_some() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn clear(&self) {
        while self.blocks.pop().is_some() {}
        self.epoch.fetch_add(1, Ordering::Release);
    }

    pub fn stats(&self) -> AudioStats {
        AudioStats {
            queued_frames: self.blocks.len() * BLOCK_FRAMES,
            dropped_blocks: self.dropped.load(Ordering::Relaxed),
            underruns: self.underruns.load(Ordering::Relaxed),
            discontinuities: self.discontinuities.load(Ordering::Relaxed),
            rendered_frames: self.rendered.load(Ordering::Relaxed),
            callback_frames: self.callback_frames.load(Ordering::Relaxed),
            callback_rate: self.callback_rate.load(Ordering::Relaxed),
            device_delay_us: self.device_delay_us.load(Ordering::Relaxed),
        }
    }

    /// OS-provided output timestamp estimate, not an acoustic measurement.
    pub fn record_device_delay(&self, delay: Option<Duration>) {
        self.device_delay_us.store(
            delay.map_or(0, |d| d.as_micros().min(u64::MAX as u128) as u64),
            Ordering::Relaxed,
        );
    }
}

pub struct AudioRenderer {
    queue: Arc<AudioQueue>,
    epoch: u64,
    block: Option<Block>,
    index: usize,
    expected_pts: Option<i64>,
    playing: bool,
    ever_played: bool,
    pair: [[f32; 2]; 2],
    phase: f64,
    rate_ratio: f64,
    last: [f32; 2],
    fade_from: [f32; 2],
    fade_remaining: usize,
    waiting_since: Option<Instant>,
    last_queued: usize,
    batch_frames: usize,
}

impl AudioRenderer {
    pub fn new(queue: Arc<AudioQueue>) -> Self {
        Self {
            epoch: queue.epoch.load(Ordering::Acquire),
            queue,
            block: None,
            index: 0,
            expected_pts: None,
            playing: false,
            ever_played: false,
            pair: [[0.; 2]; 2],
            phase: 0.,
            rate_ratio: 1.,
            last: [0.; 2],
            fade_from: [0.; 2],
            fade_remaining: 0,
            waiting_since: None,
            last_queued: 0,
            batch_frames: BLOCK_FRAMES,
        }
    }

    fn fade(&mut self) {
        self.fade_from = self.last;
        self.fade_remaining = FADE_FRAMES;
    }

    fn load(&mut self, now: Instant) -> bool {
        if self.block.as_ref().is_some_and(|b| {
            let next_sample_at = b.source_at
                + Duration::from_nanos(self.index as u64 * 1_000_000_000 / u64::from(RATE));
            self.index < b.frames && now.saturating_duration_since(next_sample_at) <= MAX_AUDIO_AGE
        }) {
            return true;
        }
        if self.block.take().is_some_and(|b| self.index < b.frames) {
            // A stalled callback may expire half of a block. Its successor can
            // still have contiguous PTS, so explicitly fade this partial skip.
            self.queue.dropped.fetch_add(1, Ordering::Relaxed);
            self.queue.discontinuities.fetch_add(1, Ordering::Relaxed);
            self.fade();
        }
        while let Some(block) = self.queue.blocks.pop() {
            if now.saturating_duration_since(block.source_at) > MAX_AUDIO_AGE {
                self.queue.dropped.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            if block.discontinuity
                || self
                    .expected_pts
                    .is_some_and(|pts| block.pts_us.saturating_sub(pts).unsigned_abs() > 1_000)
            {
                self.queue.discontinuities.fetch_add(1, Ordering::Relaxed);
                self.fade();
            }
            self.expected_pts = Some(
                block
                    .pts_us
                    .saturating_add(block.frames as i64 * 1_000_000 / i64::from(RATE)),
            );
            self.index = 0;
            self.block = Some(block);
            return true;
        }
        false
    }

    fn source_sample(&mut self, now: Instant) -> Option<[f32; 2]> {
        if !self.load(now) {
            return None;
        }
        let b = self.block.as_ref().unwrap();
        let i = self.index * CHANNELS;
        let value = [
            f32::from(b.samples[i]) / 32768.,
            f32::from(b.samples[i + 1]) / 32768.,
        ];
        self.index += 1;
        Some(value)
    }

    /// Called only by the output-device callback. `write` receives stereo frames
    /// and can convert directly into the device's native sample format.
    pub fn render(
        &mut self,
        frames: usize,
        output_rate: u32,
        now: Instant,
        mut write: impl FnMut(usize, [f32; 2]),
    ) {
        self.queue
            .callback_frames
            .store(frames as u64, Ordering::Relaxed);
        self.queue
            .callback_rate
            .store(u64::from(output_rate), Ordering::Relaxed);
        let epoch = self.queue.epoch.load(Ordering::Acquire);
        if epoch != self.epoch {
            self.epoch = epoch;
            self.block = None;
            self.expected_pts = None;
            self.playing = false;
            self.ever_played = false;
            self.phase = 0.;
            self.rate_ratio = 1.;
            self.waiting_since = None;
            self.last_queued = 0;
            self.batch_frames = BLOCK_FRAMES;
            self.fade();
        }
        let queued = self.queue.blocks.len() * BLOCK_FRAMES
            + self
                .block
                .as_ref()
                .map_or(0, |b| b.frames.saturating_sub(self.index));
        if let Some(arrived) = queued.checked_sub(self.last_queued).filter(|n| *n > 0) {
            self.batch_frames = arrived.clamp(BLOCK_FRAMES, 6 * BLOCK_FRAMES);
        }
        // A larger normal capture batch makes a larger water-level sawtooth.
        // Its midpoint is not clock drift. Keep the same safety reserve while
        // correcting only the average offset, at no more than 0.2%.
        let target = TARGET_FRAMES + (self.batch_frames - BLOCK_FRAMES) / 2;
        let desired = 1. + ((queued as f64 - target as f64) / 48_000. * 0.02).clamp(-0.002, 0.002);
        let slew = (frames as f64 / f64::from(output_rate.max(1)) / 2.).min(1.);
        self.rate_ratio += (desired - self.rate_ratio) * slew;
        let step = f64::from(RATE) / f64::from(output_rate.max(1)) * self.rate_ratio;

        for i in 0..frames {
            if !self.playing && self.load(now) {
                let b = self.block.as_ref().unwrap();
                let waited = now.saturating_duration_since(*self.waiting_since.get_or_insert(now));
                // Independent capture and device clocks can turn a 21.3 ms
                // capture callback into a 30 ms delivery gap. Prime three
                // periods before starting, then let the bounded servo settle.
                // Rebuffering must not itself age already delayed sound past
                // the hard deadline. Reserve one source period for playback;
                // this also covers device callbacks that do not divide 10 ms.
                let source_at = b.source_at
                    + Duration::from_nanos(self.index as u64 * 1_000_000_000 / u64::from(RATE));
                let approaching_expiry = now.saturating_duration_since(source_at)
                    >= MAX_AUDIO_AGE.saturating_sub(Duration::from_millis(10));
                let ready = approaching_expiry
                    || waited >= Duration::from_millis(30)
                        && (self.queue.blocks.len() * BLOCK_FRAMES + b.frames - self.index
                            >= TARGET_FRAMES
                            || waited >= Duration::from_millis(40));
                if ready {
                    self.pair[0] = self.source_sample(now).unwrap_or([0.; 2]);
                    self.pair[1] = self.source_sample(now).unwrap_or(self.pair[0]);
                    self.phase = 0.;
                    self.playing = true;
                    self.ever_played = true;
                    self.fade();
                }
            }
            let mut value = [0.; 2];
            if self.playing {
                for (ch, out) in value.iter_mut().enumerate() {
                    *out = self.pair[0][ch]
                        + (self.pair[1][ch] - self.pair[0][ch]) * self.phase as f32;
                }
                self.phase += step;
                while self.phase >= 1. {
                    self.phase -= 1.;
                    self.pair[0] = self.pair[1];
                    match self.source_sample(now) {
                        Some(next) => self.pair[1] = next,
                        None => {
                            self.playing = false;
                            self.waiting_since = None;
                            self.queue.underruns.fetch_add(1, Ordering::Relaxed);
                            self.fade();
                            break;
                        }
                    }
                }
            }
            if self.fade_remaining > 0 {
                let weight = self.fade_remaining as f32 / FADE_FRAMES as f32;
                for (ch, out) in value.iter_mut().enumerate() {
                    *out = self.fade_from[ch] * weight + *out * (1. - weight);
                }
                self.fade_remaining -= 1;
            }
            self.last = value;
            write(i, value);
        }
        self.last_queued = self.queue.blocks.len() * BLOCK_FRAMES
            + self
                .block
                .as_ref()
                .map_or(0, |b| b.frames.saturating_sub(self.index));
        if self.ever_played {
            self.queue
                .rendered
                .fetch_add(frames as u64, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests;
