//! ScreenCaptureKit screen frame + system audio capture (FR-10/20).
//!
//! Video output is BGRA (stride = width * 4); audio output is 48kHz stereo i16,
//! chunked to Opus frame size (960 samples/channel) and delivered over a channel
//! to the host send loop.
//! Requires Screen Recording TCC permission (requirements.md §6).

use crate::sck_ffi as ffi;
use core_foundation::base::TCFType;
use core_foundation::error::CFError;
use core_media_rs::cm_sample_buffer::CMSampleBuffer;
use core_media_rs::cm_time::CMTime;
use screencapturekit::{
    shareable_content::SCShareableContent,
    stream::{
        SCStream,
        configuration::{SCStreamConfiguration, pixel_format::PixelFormat},
        content_filter::SCContentFilter,
        delegate_trait::SCStreamDelegateTrait,
        output_trait::SCStreamOutputTrait,
        output_type::SCStreamOutputType,
    },
};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::mpsc;

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("screencapturekit: {0}")]
    Sc(String),
    #[error("display {0} not found")]
    DisplayNotFound(u32),
}

impl From<CFError> for CaptureError {
    fn from(e: CFError) -> Self {
        CaptureError::Sc(e.to_string())
    }
}

const SAMPLES_PER_FRAME: usize = 960; // 10ms @48kHz
const CHANNELS: usize = 2;
/// Duration of one audio frame in microseconds (960 samples @ 48 kHz).
const FRAME_DURATION_US: i64 = SAMPLES_PER_FRAME as i64 * 1_000_000 / 48_000;
/// Minimum interval between audio drop warnings.
const DROP_WARN_INTERVAL: Duration = Duration::from_secs(5);

/// One audio frame delivered to the host: 10 ms of 48 kHz stereo i16 samples
/// plus its presentation timestamp in microseconds (same epoch as the video pts).
pub struct AudioFrame {
    pub samples: Vec<i16>,
    pub pts_micros: i64,
}

/// Buffers f32 samples and emits fixed-size i16 frames with continuous
/// presentation timestamps.
struct AudioChunker {
    remainder: Vec<f32>,
    /// pts of remainder[0]; advances by FRAME_DURATION_US per emitted frame.
    next_pts_micros: i64,
}

impl AudioChunker {
    fn new() -> Self {
        Self {
            remainder: Vec::new(),
            next_pts_micros: 0,
        }
    }

    /// Appends one buffer's samples. `pts_micros` is the buffer's presentation
    /// timestamp; it is adopted only when no leftover samples are buffered,
    /// keeping the emitted timestamps on the source timeline.
    fn push(&mut self, samples: &[f32], pts_micros: Option<i64>) {
        if self.remainder.is_empty()
            && let Some(pts) = pts_micros
        {
            self.next_pts_micros = pts;
        }
        self.remainder.extend_from_slice(samples);
    }

    /// Pops the next 10 ms frame, if enough samples are buffered.
    fn pop_frame(&mut self) -> Option<AudioFrame> {
        if self.remainder.len() < SAMPLES_PER_FRAME * CHANNELS {
            return None;
        }
        let samples = self
            .remainder
            .drain(..SAMPLES_PER_FRAME * CHANNELS)
            .map(|f| (f.clamp(-1.0, 1.0) * 32767.0) as i16)
            .collect();
        let pts_micros = self.next_pts_micros;
        self.next_pts_micros += FRAME_DURATION_US;
        Some(AudioFrame {
            samples,
            pts_micros,
        })
    }
}

struct AudioState {
    chunker: AudioChunker,
    dropped_frames: u64,
    last_drop_warn: Option<Instant>,
}

struct SckOutput {
    video_tx: removent_core::latest::Sender<(Vec<u8>, i64)>,
    /// None for the screen-only output (audio capture disabled or this handler
    /// is the video one); audio callbacks are ignored then.
    audio_tx: Option<mpsc::Sender<AudioFrame>>,
    audio: Mutex<AudioState>,
}

unsafe fn pixel_ptr(sb: &CMSampleBuffer) -> Option<ffi::CVPixelBufferRef> {
    // SAFETY: sb is valid; the returned reference is owned by sb and used only
    // during the read-only lock.
    let p = unsafe { ffi::CMSampleBufferGetImageBuffer(sb.as_concrete_TypeRef().cast()) };
    (!p.is_null()).then_some(p)
}

/// kCMTimeFlags_Valid: timestamp-valid bit.
const K_CM_TIME_FLAGS_VALID: u32 = 1;

unsafe fn pts_us(sb: &CMSampleBuffer) -> Option<i64> {
    let t = unsafe { ffi::CMSampleBufferGetPresentationTimeStamp(sb.as_concrete_TypeRef().cast()) };
    if t.flags & K_CM_TIME_FLAGS_VALID == 0 || t.timescale == 0 {
        None
    } else {
        // i128 intermediate to avoid overflow: with the mach timebase, value can
        // reach 1e15+, so multiplying by 1e6 directly would overflow.
        Some((t.value as i128 * 1_000_000 / i128::from(t.timescale)) as i64)
    }
}

/// Copies a screen frame to BGRA (row by row, honoring the row stride).
fn copy_screen_bgra(sb: &CMSampleBuffer) -> Option<(Vec<u8>, i64)> {
    let pb = unsafe { pixel_ptr(sb) }?;
    let w = unsafe { ffi::CVPixelBufferGetWidth(pb) };
    let h = unsafe { ffi::CVPixelBufferGetHeight(pb) };

    // SAFETY: copies w*h*4 bytes while the read-only lock is held.
    unsafe {
        if ffi::CVPixelBufferLockBaseAddress(pb, ffi::LOCK_READ_ONLY) != 0 {
            return None;
        }
    }
    let base = unsafe { ffi::CVPixelBufferGetBaseAddress(pb) } as *const u8;
    if base.is_null() {
        unsafe { ffi::CVPixelBufferUnlockBaseAddress(pb, ffi::LOCK_READ_ONLY) };
        tracing::warn!("pixel buffer base address is null; dropping frame");
        return None;
    }
    let bpr = unsafe { ffi::CVPixelBufferGetBytesPerRow(pb) };
    let mut bgra = vec![0u8; w * h * 4];
    for row in 0..h {
        let src = unsafe { base.add(row * bpr) };
        let dst = unsafe { bgra.as_mut_ptr().add(row * w * 4) };
        unsafe {
            std::ptr::copy_nonoverlapping(src, dst, w * 4);
        }
    }
    unsafe { ffi::CVPixelBufferUnlockBaseAddress(pb, ffi::LOCK_READ_ONLY) };

    let pts = unsafe { pts_us(sb) }?;
    Some((bgra, pts))
}

/// Validates that the audio buffer is 32-bit float interleaved LPCM
/// (returns None on unexpected formats).
fn audio_is_f32_interleaved(sb: &CMSampleBuffer) -> bool {
    // SAFETY: sb is valid; the returned reference is owned by sb, read-only use.
    let desc = unsafe { ffi::CMSampleBufferGetFormatDescription(sb.as_concrete_TypeRef().cast()) };
    if desc.is_null() {
        return false;
    }
    let asbd = unsafe { ffi::CMAudioFormatDescriptionGetStreamBasicDescription(desc) };
    if asbd.is_null() {
        return false;
    }
    let asbd = unsafe { &*asbd };
    asbd.format_id == ffi::K_AUDIO_FORMAT_LINEAR_PCM
        && asbd.bits_per_channel == 32
        && asbd.channels_per_frame == CHANNELS as u32
        && asbd.format_flags & ffi::K_AUDIO_FORMAT_FLAG_IS_FLOAT != 0
        && asbd.format_flags & ffi::K_AUDIO_FORMAT_FLAG_IS_NON_INTERLEAVED == 0
}

fn copy_audio_samples(sb: &CMSampleBuffer) -> Option<Vec<f32>> {
    use crate::sck_ffi::CMSampleBufferGetDataBuffer;
    if !audio_is_f32_interleaved(sb) {
        // Warn only once on format mismatch to avoid per-frame log spam.
        static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            tracing::warn!("audio sample buffer is not f32 interleaved LPCM; skipping audio");
        }
        return None;
    }
    // SAFETY: valid sample buffer; the data block reference is owned by sb.
    let bb = unsafe { CMSampleBufferGetDataBuffer(sb.as_concrete_TypeRef().cast()) };
    if bb.is_null() {
        return None;
    }
    let len = unsafe { ffi::CMBlockBufferGetDataLength(bb) }; // byte count of f32 interleaved data
    let mut bytes = vec![0u8; len];
    unsafe { ffi::CMBlockBufferCopyDataBytes(bb, 0, len, bytes.as_mut_ptr().cast()) };
    Some(
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
    )
}

impl SckOutput {
    fn on_video(&self, sb: &CMSampleBuffer) {
        let Some((bgra, pts)) = copy_screen_bgra(sb) else {
            return;
        };
        let _ = self.video_tx.send((bgra, pts));
    }

    fn on_audio(&self, sb: &CMSampleBuffer) {
        let Some(audio_tx) = self.audio_tx.as_ref() else {
            return;
        };
        let Some(floats) = copy_audio_samples(sb) else {
            return;
        };
        let pts = unsafe { pts_us(sb) };
        let mut st = self.audio.lock().unwrap();
        st.chunker.push(&floats, pts);
        while let Some(frame) = st.chunker.pop_frame() {
            if audio_tx.try_send(frame).is_err() {
                st.dropped_frames += 1;
                let due = st
                    .last_drop_warn
                    .is_none_or(|t| t.elapsed() >= DROP_WARN_INTERVAL);
                if due {
                    tracing::warn!(
                        dropped = st.dropped_frames,
                        "audio channel full; dropped frames"
                    );
                    st.dropped_frames = 0;
                    st.last_drop_warn = Some(Instant::now());
                }
            }
        }
    }
}

impl SCStreamOutputTrait for SckOutput {
    fn did_output_sample_buffer(&self, sample_buffer: CMSampleBuffer, of_type: SCStreamOutputType) {
        match of_type {
            SCStreamOutputType::Screen => self.on_video(&sample_buffer),
            SCStreamOutputType::Audio => self.on_audio(&sample_buffer),
        }
    }
}

/// Stream delegate: reports fatal stream errors (lock screen, fast user
/// switch, revoked permission, disconnected display) to the session owner.
struct StoppedDelegate {
    stopped_tx: mpsc::Sender<String>,
}

impl SCStreamDelegateTrait for StoppedDelegate {
    fn did_stop_with_error(&self, _stream: SCStream, error: CFError) {
        let msg = error.to_string();
        tracing::error!(error = %msg, "screen capture stream stopped with error");
        let _ = self.stopped_tx.try_send(msg);
    }
}

/// A running capture session; stops the stream on drop.
pub struct SckCapture {
    stream: SCStream,
    stopped_rx: Option<mpsc::Receiver<String>>,
}

impl SckCapture {
    /// Takes the channel notified once when the capture stream stops with an
    /// error. Returns `None` if the receiver was already taken.
    pub fn take_stopped_rx(&mut self) -> Option<mpsc::Receiver<String>> {
        self.stopped_rx.take()
    }
}

impl Drop for SckCapture {
    fn drop(&mut self) {
        let _ = self.stream.stop_capture();
    }
}

/// Starts screen capture for the given display, plus system audio capture when
/// `audio_tx` is Some (a peer that declined audio gets no audio pipeline at all).
/// Width/height are the encoder input dimensions.
pub fn start_display_capture(
    display_id: u32,
    width: u32,
    height: u32,
    video_tx: removent_core::latest::Sender<(Vec<u8>, i64)>,
    audio_tx: Option<mpsc::Sender<AudioFrame>>,
) -> Result<SckCapture, CaptureError> {
    let content = SCShareableContent::get().map_err(|e| CaptureError::Sc(e.to_string()))?;
    let display = content
        .displays()
        .into_iter()
        .find(|d| d.display_id() == display_id)
        .ok_or(CaptureError::DisplayNotFound(display_id))?;

    let filter = SCContentFilter::new();
    // Exclude the Removent app's windows (matched by bundle id) to avoid
    // recursive capture of the viewer window; also exclude this process's own
    // windows (harmless for the headless daemon, but keeps the guard correct
    // if the capture code ever runs in-process with the app)
    // (architecture.md §3.1).
    let own_pid = std::process::id() as i32;
    let own_windows: Vec<_> = content
        .windows()
        .into_iter()
        .filter(|w| {
            let app = w.owning_application();
            app.process_id() == own_pid || app.bundle_identifier() == "com.alkinum.removent"
        })
        .collect();
    let own_window_refs: Vec<&_> = own_windows.iter().collect();
    let filter = filter.with_display_excluding_windows(&display, &own_window_refs);
    let frame_interval_60fps = CMTime {
        value: 1,
        timescale: 60,
        flags: 1,
        epoch: 0,
    };
    let config = SCStreamConfiguration::new();
    let config = match config
        .set_width(width)
        .and_then(|c| c.set_height(height))
        .and_then(|c| c.set_pixel_format(PixelFormat::BGRA))
        .and_then(|c| c.set_queue_depth(3))
        .and_then(|c| c.set_minimum_frame_interval(&frame_interval_60fps))
        .and_then(|c| c.set_captures_audio(audio_tx.is_some()))
        .and_then(|c| c.set_excludes_current_process_audio(true))
        .and_then(|c| c.set_sample_rate(48_000))
        .and_then(|c| c.set_channel_count(2))
    {
        Ok(c) => c,
        Err(e) => return Err(CaptureError::Sc(e.to_string())),
    };

    let (stopped_tx, stopped_rx) = mpsc::channel(1);
    let mut stream = SCStream::new_with_delegate(&filter, &config, StoppedDelegate { stopped_tx });
    let screen_output = SckOutput {
        video_tx: video_tx.clone(),
        audio_tx: None,
        audio: Mutex::new(AudioState {
            chunker: AudioChunker::new(),
            dropped_frames: 0,
            last_drop_warn: None,
        }),
    };

    stream.add_output_handler(screen_output, SCStreamOutputType::Screen);
    if let Some(audio_tx) = audio_tx {
        let audio_output = SckOutput {
            video_tx,
            audio_tx: Some(audio_tx),
            audio: Mutex::new(AudioState {
                chunker: AudioChunker::new(),
                dropped_frames: 0,
                last_drop_warn: None,
            }),
        };
        stream.add_output_handler(audio_output, SCStreamOutputType::Audio);
    }

    stream.start_capture().map_err(CaptureError::from)?;
    Ok(SckCapture {
        stream,
        stopped_rx: Some(stopped_rx),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Samples below one frame are buffered; the frame adopts the pts of the
    /// buffer that started it.
    #[test]
    fn chunker_buffers_partial_and_adopts_pts() {
        let mut c = AudioChunker::new();
        c.push(&vec![0.0f32; SAMPLES_PER_FRAME * CHANNELS / 2], Some(5000));
        assert!(c.pop_frame().is_none());
        // The remainder is non-empty, so this buffer's pts must not be adopted.
        c.push(&vec![0.5f32; SAMPLES_PER_FRAME * CHANNELS / 2], Some(9999));
        let frame = c.pop_frame().expect("one full frame");
        assert_eq!(frame.samples.len(), SAMPLES_PER_FRAME * CHANNELS);
        assert_eq!(frame.pts_micros, 5000);
    }

    /// pts advances by exactly one frame duration (20 ms) per emitted frame.
    #[test]
    fn chunker_pts_advances_per_frame() {
        let mut c = AudioChunker::new();
        c.push(
            &vec![1.0f32; 2 * SAMPLES_PER_FRAME * CHANNELS],
            Some(123_456),
        );
        let f0 = c.pop_frame().unwrap();
        let f1 = c.pop_frame().unwrap();
        assert_eq!(f0.pts_micros, 123_456);
        assert_eq!(f1.pts_micros, 123_456 + FRAME_DURATION_US);
        assert!(c.pop_frame().is_none());
    }

    /// A buffer arriving with an empty remainder re-anchors the timeline.
    #[test]
    fn chunker_reanchors_on_empty_remainder() {
        let mut c = AudioChunker::new();
        c.push(&vec![0.0f32; SAMPLES_PER_FRAME * CHANNELS], Some(1000));
        let _ = c.pop_frame().unwrap();
        c.push(&vec![0.0f32; SAMPLES_PER_FRAME * CHANNELS], Some(777_000));
        assert_eq!(c.pop_frame().unwrap().pts_micros, 777_000);
    }

    /// An invalid (missing) pts keeps the running timeline instead of
    /// resetting it.
    #[test]
    fn chunker_invalid_pts_keeps_continuity() {
        let mut c = AudioChunker::new();
        c.push(&vec![0.0f32; SAMPLES_PER_FRAME * CHANNELS], Some(40_000));
        let _ = c.pop_frame().unwrap();
        c.push(&vec![0.0f32; SAMPLES_PER_FRAME * CHANNELS], None);
        assert_eq!(
            c.pop_frame().unwrap().pts_micros,
            40_000 + FRAME_DURATION_US
        );
    }

    /// Samples are clamped and scaled to i16 full scale.
    #[test]
    fn chunker_clamps_to_i16() {
        let mut c = AudioChunker::new();
        let mut samples = vec![0.0f32; SAMPLES_PER_FRAME * CHANNELS];
        samples[0] = 2.0;
        samples[1] = -2.0;
        samples[2] = 1.0;
        c.push(&samples, Some(0));
        let frame = c.pop_frame().unwrap();
        assert_eq!(frame.samples[0], 32767);
        assert_eq!(frame.samples[1], -32767);
        assert_eq!(frame.samples[2], 32767);
    }
}
