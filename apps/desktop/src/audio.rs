//! Low-latency system audio output for decoded 48 kHz stereo PCM.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Data, SampleFormat};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

const INPUT_RATE: f64 = 48_000.0;
const CHANNELS: usize = 2;
const MAX_QUEUE_FRAMES: usize = 9_600;

struct OutputState {
    samples: VecDeque<f32>,
    source_phase: f64,
}

impl OutputState {
    fn push(&mut self, pcm: Vec<i16>) {
        if !pcm.len().is_multiple_of(CHANNELS) {
            tracing::warn!(samples = pcm.len(), "dropping malformed stereo PCM frame");
            return;
        }
        self.samples
            .extend(pcm.into_iter().map(|s| f32::from(s) / f32::from(i16::MAX)));
        let max_samples = MAX_QUEUE_FRAMES * CHANNELS;
        while self.samples.len() > max_samples {
            self.samples.pop_front();
        }
    }

    fn render(&mut self, out: &mut [f32], output_channels: usize, output_rate: u32) {
        let step = INPUT_RATE / f64::from(output_rate.max(1));
        let frames = out.len() / output_channels.max(1);
        for frame_idx in 0..frames {
            let source_idx = self.source_phase.floor() as usize;
            let frac = (self.source_phase - source_idx as f64) as f32;
            let available = self.samples.len() / CHANNELS;
            let (left, right) = if source_idx + 1 < available {
                let i = source_idx * CHANNELS;
                let next = i + CHANNELS;
                let l = self.samples[i] * (1.0 - frac) + self.samples[next] * frac;
                let r = self.samples[i + 1] * (1.0 - frac) + self.samples[next + 1] * frac;
                (l, r)
            } else if source_idx < available {
                let i = source_idx * CHANNELS;
                (self.samples[i], self.samples[i + 1])
            } else {
                (0.0, 0.0)
            };
            let base = frame_idx * output_channels;
            for ch in 0..output_channels {
                out[base + ch] = if output_channels == 1 {
                    (left + right) * 0.5
                } else if ch.is_multiple_of(2) {
                    left
                } else {
                    right
                };
            }
            self.source_phase += step;
            let consumed = self.source_phase.floor() as usize;
            if consumed > 0 {
                let drop = (consumed * CHANNELS).min(self.samples.len());
                self.samples.drain(..drop);
                self.source_phase -= consumed as f64;
            }
        }
        // A partial device buffer (unusual, but valid) is silenced.
        for sample in &mut out[frames * output_channels..] {
            *sample = 0.0;
        }
    }
}

struct AudioThread {
    state: Arc<Mutex<OutputState>>,
    shutdown: std::sync::mpsc::Sender<()>,
}

impl Drop for AudioThread {
    fn drop(&mut self) {
        let _ = self.shutdown.send(());
    }
}

/// Thread-safe producer handle. The CPAL stream stays on the dedicated thread
/// that created it because streams are deliberately thread-affine.
#[derive(Clone)]
pub struct AudioPlayer {
    inner: Arc<AudioThread>,
}

impl AudioPlayer {
    pub fn new() -> Result<Self, String> {
        let state = Arc::new(Mutex::new(OutputState {
            samples: VecDeque::with_capacity(MAX_QUEUE_FRAMES * CHANNELS),
            source_phase: 0.0,
        }));
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel();
        let thread_state = state.clone();
        std::thread::Builder::new()
            .name("audio-output".into())
            .spawn(move || match open_stream(thread_state) {
                Ok(stream) => {
                    let _ = ready_tx.send(Ok(()));
                    let _ = shutdown_rx.recv();
                    drop(stream);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            })
            .map_err(|e| format!("start audio output thread: {e}"))?;
        ready_rx
            .recv()
            .map_err(|_| "audio output thread exited during startup".to_string())??;
        Ok(Self {
            inner: Arc::new(AudioThread {
                state,
                shutdown: shutdown_tx,
            }),
        })
    }

    pub fn clear(&self) {
        if let Ok(mut state) = self.inner.state.lock() {
            state.samples.clear();
            state.source_phase = 0.0;
        }
    }

    pub fn push(&self, pcm: Vec<i16>) {
        if pcm.len() < CHANNELS || !pcm.len().is_multiple_of(CHANNELS) {
            return;
        }
        if let Ok(mut state) = self.inner.state.lock() {
            state.push(pcm);
        }
    }
}

fn open_stream(state: Arc<Mutex<OutputState>>) -> Result<cpal::Stream, String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "no default audio output device".to_string())?;
    let supported = device
        .default_output_config()
        .map_err(|e| format!("default output config: {e}"))?;
    let sample_format = supported.sample_format();
    let channels = usize::from(supported.channels());
    let rate = supported.sample_rate().0;
    let config: cpal::StreamConfig = supported.into();
    let callback_state = state.clone();
    let mut scratch = Vec::<f32>::new();
    let err_fn = |err| tracing::warn!(err = %err, "audio output stream error");
    let stream = device
        .build_output_stream_raw(
            &config,
            sample_format,
            move |data: &mut Data, _| {
                scratch.resize(data.len(), 0.0);
                scratch.fill(0.0);
                if let Ok(mut state) = callback_state.lock() {
                    state.render(&mut scratch, channels, rate);
                }
                write_samples(data, &scratch);
            },
            err_fn,
            None,
        )
        .map_err(|e| format!("build audio output stream: {e}"))?;
    stream
        .play()
        .map_err(|e| format!("start audio output stream: {e}"))?;
    tracing::info!(device = ?device.name().ok(), channels, rate, ?sample_format, "audio output started");
    Ok(stream)
}

fn write_samples(data: &mut Data, samples: &[f32]) {
    match data.sample_format() {
        SampleFormat::F32 => copy_as(data.as_slice_mut::<f32>(), samples, |s| s),
        SampleFormat::F64 => copy_as(data.as_slice_mut::<f64>(), samples, f64::from),
        SampleFormat::I8 => copy_as(data.as_slice_mut::<i8>(), samples, |s| {
            (s.clamp(-1.0, 1.0) * f32::from(i8::MAX)) as i8
        }),
        SampleFormat::I16 => copy_as(data.as_slice_mut::<i16>(), samples, |s| {
            (s.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16
        }),
        SampleFormat::I32 => copy_as(data.as_slice_mut::<i32>(), samples, |s| {
            (s.clamp(-1.0, 1.0) * i32::MAX as f32) as i32
        }),
        SampleFormat::I64 => copy_as(data.as_slice_mut::<i64>(), samples, |s| {
            (s.clamp(-1.0, 1.0) * i64::MAX as f32) as i64
        }),
        SampleFormat::U8 => copy_as(data.as_slice_mut::<u8>(), samples, |s| {
            (((s.clamp(-1.0, 1.0) + 1.0) * 0.5) * f32::from(u8::MAX)) as u8
        }),
        SampleFormat::U16 => copy_as(data.as_slice_mut::<u16>(), samples, |s| {
            (((s.clamp(-1.0, 1.0) + 1.0) * 0.5) * f32::from(u16::MAX)) as u16
        }),
        SampleFormat::U32 => copy_as(data.as_slice_mut::<u32>(), samples, |s| {
            (((s.clamp(-1.0, 1.0) + 1.0) * 0.5) * u32::MAX as f32) as u32
        }),
        SampleFormat::U64 => copy_as(data.as_slice_mut::<u64>(), samples, |s| {
            (((s.clamp(-1.0, 1.0) + 1.0) * 0.5) * u64::MAX as f32) as u64
        }),
        _ => data.bytes_mut().fill(0),
    }
}

fn copy_as<T: Copy>(dst: Option<&mut [T]>, src: &[f32], convert: impl Fn(f32) -> T) {
    if let Some(dst) = dst {
        for (out, input) in dst.iter_mut().zip(src.iter().copied()) {
            *out = convert(input);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_render_preserves_channels() {
        let mut state = OutputState {
            samples: VecDeque::new(),
            source_phase: 0.0,
        };
        state.push(vec![i16::MAX, i16::MIN, i16::MAX / 2, i16::MIN / 2]);
        let mut out = [0.0; 4];
        state.render(&mut out, 2, 48_000);
        assert!(out[0] > 0.99 && out[1] < -0.99);
        assert!(out[2] > 0.49 && out[3] < -0.49);
    }

    #[test]
    fn mono_output_downmixes_stereo() {
        let mut state = OutputState {
            samples: VecDeque::new(),
            source_phase: 0.0,
        };
        state.push(vec![i16::MAX, 0, i16::MAX, 0]);
        let mut out = [0.0; 2];
        state.render(&mut out, 1, 48_000);
        assert!(out.iter().all(|sample| (sample - 0.5).abs() < 0.001));
    }

    #[test]
    fn queue_limit_keeps_stereo_alignment() {
        let mut state = OutputState {
            samples: VecDeque::new(),
            source_phase: 0.0,
        };
        state.push(vec![1; MAX_QUEUE_FRAMES * CHANNELS + 8]);
        assert!(state.samples.len() <= MAX_QUEUE_FRAMES * CHANNELS);
        assert_eq!(state.samples.len() % CHANNELS, 0);
    }

    #[test]
    fn malformed_pcm_is_rejected_without_shifting_channels() {
        let mut state = OutputState {
            samples: VecDeque::new(),
            source_phase: 0.0,
        };
        state.push(vec![1, 2, 3]);
        assert!(state.samples.is_empty());
    }
}
