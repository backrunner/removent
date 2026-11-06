//! Device-clocked system audio output; callbacks do not allocate or lock.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Data, SampleFormat};
use removent_client::audio::{AudioQueue, AudioRenderer, DecodedAudio};
use std::sync::Arc;
use std::time::Instant;

struct AudioThread {
    queue: Arc<AudioQueue>,
    shutdown: std::sync::mpsc::Sender<()>,
}
impl Drop for AudioThread {
    fn drop(&mut self) {
        let _ = self.shutdown.send(());
    }
}

#[derive(Clone)]
pub struct AudioPlayer {
    inner: Arc<AudioThread>,
}

impl AudioPlayer {
    pub fn new() -> Result<Self, String> {
        let queue = Arc::new(AudioQueue::default());
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel();
        let output = queue.clone();
        std::thread::Builder::new()
            .name("audio-output".into())
            .spawn(move || match open_stream(output) {
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
                queue,
                shutdown: shutdown_tx,
            }),
        })
    }
    pub fn clear(&self) {
        self.inner.queue.clear();
    }
    pub fn push(&self, pcm: DecodedAudio) {
        self.inner.queue.push(pcm);
    }
    pub fn stats(&self) -> removent_client::audio::AudioStats {
        self.inner.queue.stats()
    }
}

fn open_stream(queue: Arc<AudioQueue>) -> Result<cpal::Stream, String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "no default audio output device".to_string())?;
    let supported = device
        .default_output_config()
        .map_err(|e| format!("default output config: {e}"))?;
    let format = supported.sample_format();
    let preferred_buffer = match supported.buffer_size() {
        cpal::SupportedBufferSize::Range { min, max } if min <= max => {
            Some((supported.sample_rate() / 200).clamp(*min, *max))
        }
        _ => None,
    };
    let mut config: cpal::StreamConfig = supported.into();
    if let Some(frames) = preferred_buffer {
        config.buffer_size = cpal::BufferSize::Fixed(frames);
    }
    let channels = usize::from(config.channels);
    let rate = config.sample_rate;
    let build = |config| {
        let mut renderer = AudioRenderer::new(queue.clone());
        let stats = queue.clone();
        device.build_output_stream_raw(
            config,
            format,
            move |data: &mut Data, info| {
                let timestamp = info.timestamp();
                let delay = timestamp
                    .playback
                    .checked_duration_since(timestamp.callback);
                stats.record_device_delay(delay);
                write_samples(data, &mut renderer, channels, rate);
            },
            |error| tracing::warn!(%error, "audio output stream error"),
            None,
        )
    };
    let stream = match build(config) {
        Ok(stream) => stream,
        Err(error) if preferred_buffer.is_some() => {
            tracing::warn!(%error, "small audio buffer rejected; retrying device default");
            config.buffer_size = cpal::BufferSize::Default;
            build(config).map_err(|e| format!("build default audio output stream: {e}"))?
        }
        Err(error) => return Err(format!("build audio output stream: {error}")),
    };
    stream
        .play()
        .map_err(|e| format!("start audio output stream: {e}"))?;
    tracing::info!(device = ?device.description().ok(), channels, rate, ?format, buffer = ?config.buffer_size, "audio output started");
    Ok(stream)
}

fn write_samples(data: &mut Data, renderer: &mut AudioRenderer, channels: usize, rate: u32) {
    let now = Instant::now();
    macro_rules! output {
        ($ty:ty, $convert:expr) => {
            render_as(
                data.as_slice_mut::<$ty>(),
                renderer,
                channels,
                rate,
                now,
                $convert,
            )
        };
    }
    match data.sample_format() {
        SampleFormat::F32 => output!(f32, |s| s),
        SampleFormat::F64 => output!(f64, f64::from),
        SampleFormat::I8 => output!(i8, |s: f32| (s.clamp(-1., 1.) * i8::MAX as f32) as i8),
        SampleFormat::I16 => output!(i16, |s: f32| (s.clamp(-1., 1.) * i16::MAX as f32) as i16),
        SampleFormat::I32 => output!(i32, |s: f32| (s.clamp(-1., 1.) * i32::MAX as f32) as i32),
        SampleFormat::I64 => output!(i64, |s: f32| (s.clamp(-1., 1.) * i64::MAX as f32) as i64),
        SampleFormat::U8 => output!(
            u8,
            |s: f32| ((s.clamp(-1., 1.) + 1.) * 0.5 * u8::MAX as f32) as u8
        ),
        SampleFormat::U16 => output!(
            u16,
            |s: f32| ((s.clamp(-1., 1.) + 1.) * 0.5 * u16::MAX as f32) as u16
        ),
        SampleFormat::U32 => output!(
            u32,
            |s: f32| ((s.clamp(-1., 1.) + 1.) * 0.5 * u32::MAX as f32) as u32
        ),
        SampleFormat::U64 => output!(
            u64,
            |s: f32| ((s.clamp(-1., 1.) + 1.) * 0.5 * u64::MAX as f32) as u64
        ),
        _ => data.bytes_mut().fill(0),
    }
}

fn render_as<T: Copy>(
    dst: Option<&mut [T]>,
    renderer: &mut AudioRenderer,
    channels: usize,
    rate: u32,
    now: Instant,
    convert: impl Fn(f32) -> T,
) {
    let Some(dst) = dst else { return };
    let channels = channels.max(1);
    let frames = dst.len() / channels;
    renderer.render(frames, rate, now, |i, pair| {
        for ch in 0..channels {
            let value = if channels == 1 {
                (pair[0] + pair[1]) * 0.5
            } else {
                pair[ch % 2]
            };
            dst[i * channels + ch] = convert(value);
        }
    });
    dst[frames * channels..].fill(convert(0.));
}
