//! Video codec: VideoToolbox hardware encode/decode wrappers.
//!
//! H264/HEVC frame data is Annex-B (with start codes); AV1 frame data is a
//! complete temporal-unit OBU stream. Parameter sets are inlined before
//! H264/HEVC keyframes (H264: SPS+PPS, HEVC: VPS+SPS+PPS), while rav1e emits
//! the AV1 sequence header in keyframe temporal units.
//! The decoder extracts parameter sets from Annex-B to create the
//! CMVideoFormatDescription.

use crate::av1::{Av1Decoder, Av1Encoder};
use crate::cm_ffi as cm;
use crate::compat_decompression::CompatibleDecompressionSession;
use apple_cf::iosurface::IOSurface;
use std::os::raw::c_void;
use std::sync::{Arc, mpsc};
use videotoolbox::session::Codec;

pub const BGRA_FOURCC: u32 = u32::from_be_bytes(*b"BGRA");
/// CoreMedia four-character code for AV1 (`av01`). The software AV1 path is
/// used for media today; this constant remains useful for hardware probes.
pub const AV1_CODEC_TYPE: u32 = u32::from_be_bytes(*b"av01");
const TIMESCALE_US: i32 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Av1HardwareSupport {
    /// Whether VideoToolbox reports a hardware AV1 decoder on this Mac.
    pub decoder: bool,
    /// Whether the installed VideoToolbox encoder list contains a hardware
    /// AV1 encoder for the current OS/device.
    pub encoder: bool,
}

/// Probe VideoToolbox's AV1 hardware support without creating a session or
/// changing protocol negotiation. Software AV1 support is provided by rav1e
/// and rav1d independently of this probe.
pub fn av1_hardware_support() -> Av1HardwareSupport {
    let decoder = unsafe { videotoolbox::ffi::VTIsHardwareDecodeSupported(AV1_CODEC_TYPE) != 0 };
    let encoder = videotoolbox::available_video_encoder_details()
        .ok()
        .into_iter()
        .flatten()
        .any(|entry| {
            entry.base.codec_type == AV1_CODEC_TYPE && entry.is_hardware_accelerated == Some(true)
        });
    Av1HardwareSupport { decoder, encoder }
}

fn is_hevc(codec: removent_proto::CodecId) -> bool {
    matches!(codec, removent_proto::CodecId::Hevc)
}

#[derive(Debug, thiserror::Error)]
pub enum VideoError {
    #[error("videotoolbox: {0}")]
    Vt(#[from] videotoolbox::VTError),
    #[error("surface: {0}")]
    Surface(String),
    #[error("pixel data size mismatch: need {need}, got {got}")]
    PixelSizeMismatch { need: usize, got: usize },
    #[error("parameter sets unavailable before first encode")]
    NoParameterSets,
    #[error("core media status {0}")]
    CoreMedia(i32),
    #[error("decode failed with status {0}")]
    DecodeStatus(i32),
    #[error("core media returned null sample buffer")]
    NullSampleBuffer,
    #[error("avcc length prefix invalid")]
    InvalidAvcc,
    #[error("software AV1: {0}")]
    Av1(String),
}

/// One encoded frame (Annex-B for H264/HEVC, temporal-unit OBUs for AV1).
#[derive(Debug, Clone)]
pub struct EncodedVideoFrame {
    pub data: Vec<u8>,
    pub pts_us: i64,
    pub keyframe: bool,
}

fn vt_codec(codec: removent_proto::CodecId) -> Codec {
    match codec {
        removent_proto::CodecId::H264 => Codec::H264,
        removent_proto::CodecId::Hevc => Codec::HEVC,
        removent_proto::CodecId::Av1 => unreachable!("AV1 uses the software codec path"),
    }
}

const START_CODE: &[u8; 4] = &[0, 0, 0, 1];

/// AVCC (4-byte length-prefixed NALU sequence) → Annex-B.
pub fn avcc_to_annexb(data: &[u8]) -> Result<Vec<u8>, VideoError> {
    let mut out = Vec::with_capacity(data.len() + 16);
    let mut rest = data;
    while !rest.is_empty() {
        if rest.len() < 4 {
            return Err(VideoError::InvalidAvcc);
        }
        let nalu_len = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize;
        let total = 4 + nalu_len;
        if nalu_len == 0 || rest.len() < total {
            return Err(VideoError::InvalidAvcc);
        }
        out.extend_from_slice(START_CODE);
        out.extend_from_slice(&rest[4..total]);
        rest = &rest[total..];
    }
    Ok(out)
}

/// Splits Annex-B by start codes, returning NALUs without start codes
/// (trailing zero bytes stripped).
pub fn split_annexb_nals(data: &[u8]) -> Vec<&[u8]> {
    // Collect (NAL data start, start-code start). Consecutive zero bytes before
    // a start code belong to the separator.
    let mut marks: Vec<(usize, usize)> = Vec::new();
    let mut i = 0usize;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            let mut code_start = i;
            while code_start > 0 && data[code_start - 1] == 0 {
                code_start -= 1;
            }
            marks.push((i + 3, code_start));
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut nals = Vec::with_capacity(marks.len());
    for w in 0..marks.len() {
        let s = marks[w].0;
        let e = marks.get(w + 1).map(|m| m.1).unwrap_or(data.len());
        if e > s {
            nals.push(&data[s..e]);
        }
    }
    nals
}

fn nal_type(nal: &[u8], hevc: bool) -> u8 {
    if nal.is_empty() {
        return 0;
    }
    if hevc {
        (nal[0] >> 1) & 0x3F
    } else {
        nal[0] & 0x1F
    }
}

fn is_param_set(nal_type: u8, hevc: bool) -> bool {
    if hevc {
        matches!(nal_type, 32..=34) // VPS/SPS/PPS
    } else {
        matches!(nal_type, 7 | 8) // SPS/PPS
    }
}

fn is_idr(nal_type: u8, hevc: bool) -> bool {
    if hevc {
        matches!(nal_type, 19 | 20)
    } else {
        nal_type == 5
    }
}

/// Extracts parameter-set NALUs from Annex-B (in order of appearance).
pub fn extract_param_sets(annexb: &[u8], hevc: bool) -> Vec<Vec<u8>> {
    split_annexb_nals(annexb)
        .into_iter()
        .filter(|n| is_param_set(nal_type(n, hevc), hevc))
        .map(|n| n.to_vec())
        .collect()
}

/// Returns whether the Annex-B data contains an IDR frame.
pub fn annexb_has_idr(annexb: &[u8], hevc: bool) -> bool {
    split_annexb_nals(annexb)
        .iter()
        .any(|n| is_idr(nal_type(n, hevc), hevc))
}

// ---------------- Encoding ----------------

/// One encoded sample straight from the VT output callback: the AVCC payload
/// plus the retained sample buffer (needed for keyframe and parameter-set
/// inspection). The sample buffer reference is released on drop.
struct RawEncodedSample {
    data: Vec<u8>,
    sample_buffer: cm::CMSampleBufferRef,
}

impl Drop for RawEncodedSample {
    fn drop(&mut self) {
        // SAFETY: the output callback retained this reference for us.
        unsafe { videotoolbox::ffi::CFRelease(self.sample_buffer.cast()) };
    }
}

// SAFETY: the retained CMSampleBuffer is a CoreFoundation object, which is
// thread-safe (CFType docs); it is only read and finally released.
unsafe impl Send for RawEncodedSample {}

struct EncoderCallbackState {
    out_tx: mpsc::Sender<Result<Option<RawEncodedSample>, i32>>,
}

/// VTCompressionSession output callback: copies out the AVCC payload and
/// forwards the retained sample buffer to the blocking `encode` caller.
unsafe extern "C" fn encode_output_callback(
    output_callback_ref_con: *mut c_void,
    _source_frame_ref_con: *mut c_void,
    status: i32,
    info_flags: u32,
    sample_buffer: videotoolbox::ffi::CMSampleBufferRef,
) {
    if output_callback_ref_con.is_null() {
        return;
    }
    // SAFETY: the ref-con is the Arc<EncoderCallbackState> leaked in
    // EncoderSession::new; it stays alive until the session is invalidated.
    let state = unsafe { &*output_callback_ref_con.cast::<EncoderCallbackState>() };
    let msg = if status != 0 {
        Err(status)
    } else if sample_buffer.is_null()
        || info_flags & videotoolbox::ffi::kVTEncodeInfo_FrameDropped != 0
    {
        Ok(None) // the encoder dropped the frame
    } else {
        // SAFETY: sample_buffer is valid for the duration of the callback; the
        // block buffer reference is owned by it.
        let block = unsafe { videotoolbox::ffi::CMSampleBufferGetDataBuffer(sample_buffer) };
        if block.is_null() {
            Err(-1)
        } else {
            let len = unsafe { videotoolbox::ffi::CMBlockBufferGetDataLength(block) };
            let mut data = vec![0u8; len];
            let copy = unsafe {
                videotoolbox::ffi::CMBlockBufferCopyDataBytes(
                    block,
                    0,
                    len,
                    data.as_mut_ptr().cast(),
                )
            };
            if copy != 0 {
                Err(copy)
            } else {
                // SAFETY: retain so the buffer outlives the callback; released
                // by RawEncodedSample::drop.
                unsafe { videotoolbox::ffi::CFRetain(sample_buffer.cast()) };
                Ok(Some(RawEncodedSample {
                    data,
                    sample_buffer: sample_buffer.cast(),
                }))
            }
        }
    };
    let _ = state.out_tx.send(msg);
}

/// VideoToolbox compression session driven through FFI directly.
///
/// The videotoolbox crate's `CompressionSession::encode` accepts no per-frame
/// properties and does not expose the raw session handle, but ForceKeyFrame is
/// a frame-level option (`kVTEncodeFrameOptionKey_ForceKeyFrame`) that must be
/// passed to `VTCompressionSessionEncodeFrame` — setting it via
/// `VTSessionSetProperty` fails with kVTPropertyNotSupportedErr.
struct EncoderSession {
    session: videotoolbox::ffi::VTCompressionSessionRef,
    /// Leaked `Arc<EncoderCallbackState>` handed to the callback; reclaimed on drop.
    callback_state: *mut c_void,
    out_rx: mpsc::Receiver<Result<Option<RawEncodedSample>, i32>>,
}

// SAFETY: VideoToolbox sessions are documented as thread-safe; the output
// callback only touches the channel sender.
unsafe impl Send for EncoderSession {}

impl EncoderSession {
    fn new(
        codec: Codec,
        width: i32,
        height: i32,
        bitrate_kbps: u32,
        fps: u8,
    ) -> Result<Self, VideoError> {
        let (tx, rx) = mpsc::channel();
        let callback_state = Arc::into_raw(Arc::new(EncoderCallbackState { out_tx: tx }))
            .cast_mut()
            .cast::<c_void>();
        let mut session: videotoolbox::ffi::VTCompressionSessionRef = std::ptr::null_mut();
        // SAFETY: all pointers are valid; callback_state is an Arc leaked for
        // the session lifetime and reclaimed on failure below or in Drop.
        let status = unsafe {
            videotoolbox::ffi::VTCompressionSessionCreate(
                videotoolbox::ffi::kCFAllocatorDefault,
                width,
                height,
                codec.as_cm_codec_type(),
                std::ptr::null(),
                std::ptr::null(),
                videotoolbox::ffi::kCFAllocatorDefault,
                Some(encode_output_callback),
                callback_state,
                &mut session,
            )
        };
        if status != 0 || session.is_null() {
            // SAFETY: created above via Arc::into_raw; the session never took it.
            unsafe { drop(Arc::from_raw(callback_state.cast::<EncoderCallbackState>())) };
            return Err(VideoError::CoreMedia(status));
        }
        let s = Self {
            session,
            callback_state,
            out_rx: rx,
        };
        s.set_bool_property(
            unsafe { videotoolbox::ffi::kVTCompressionPropertyKey_RealTime },
            true,
        )?;
        s.set_bool_property(
            unsafe { videotoolbox::ffi::kVTCompressionPropertyKey_AllowFrameReordering },
            false,
        )?;
        s.set_i32_property(
            unsafe { videotoolbox::ffi::kVTCompressionPropertyKey_AverageBitRate },
            (bitrate_kbps * 1000) as i32,
        )?;
        s.set_f64_property(
            unsafe { videotoolbox::ffi::kVTCompressionPropertyKey_ExpectedFrameRate },
            f64::from(fps),
        )?;
        s.set_i32_property(
            unsafe { videotoolbox::ffi::kVTCompressionPropertyKey_MaxKeyFrameInterval },
            i32::from(fps) * 2,
        )?;
        // SAFETY: session is valid.
        let status =
            unsafe { videotoolbox::ffi::VTCompressionSessionPrepareToEncodeFrames(session) };
        if status != 0 {
            return Err(VideoError::CoreMedia(status));
        }
        Ok(s)
    }

    fn set_bool_property(
        &self,
        key: videotoolbox::ffi::CFStringRef,
        value: bool,
    ) -> Result<(), VideoError> {
        // SAFETY: extern constant booleans are process-lifetime objects.
        let v = unsafe {
            if value {
                videotoolbox::ffi::kCFBooleanTrue
            } else {
                videotoolbox::ffi::kCFBooleanFalse
            }
        };
        // SAFETY: session and key are valid; value is a CFBoolean.
        let status =
            unsafe { videotoolbox::ffi::VTSessionSetProperty(self.session, key, v.cast()) };
        if status != 0 {
            return Err(VideoError::CoreMedia(status));
        }
        Ok(())
    }

    fn set_i32_property(
        &self,
        key: videotoolbox::ffi::CFStringRef,
        value: i32,
    ) -> Result<(), VideoError> {
        // SAFETY: value outlives the SetProperty call; the number is released
        // right after.
        unsafe {
            let num = videotoolbox::ffi::CFNumberCreate(
                videotoolbox::ffi::kCFAllocatorDefault,
                videotoolbox::ffi::kCFNumberSInt32Type,
                std::ptr::from_ref(&value).cast(),
            );
            let status = videotoolbox::ffi::VTSessionSetProperty(self.session, key, num.cast());
            videotoolbox::ffi::CFRelease(num.cast());
            if status != 0 {
                return Err(VideoError::CoreMedia(status));
            }
        }
        Ok(())
    }

    fn set_f64_property(
        &self,
        key: videotoolbox::ffi::CFStringRef,
        value: f64,
    ) -> Result<(), VideoError> {
        // SAFETY: value outlives the SetProperty call; the number is released
        // right after.
        unsafe {
            let num = videotoolbox::ffi::CFNumberCreate(
                videotoolbox::ffi::kCFAllocatorDefault,
                videotoolbox::ffi::kCFNumberFloat64Type,
                std::ptr::from_ref(&value).cast(),
            );
            let status = videotoolbox::ffi::VTSessionSetProperty(self.session, key, num.cast());
            videotoolbox::ffi::CFRelease(num.cast());
            if status != 0 {
                return Err(VideoError::CoreMedia(status));
            }
        }
        Ok(())
    }

    /// Submits one surface and blocks until the encoder emits the output.
    /// When `force_keyframe` is set, the frame-level ForceKeyFrame option is
    /// attached. `Ok(None)` means the encoder dropped the frame.
    fn encode(
        &self,
        surface: &IOSurface,
        pts_us: i64,
        force_keyframe: bool,
    ) -> Result<Option<RawEncodedSample>, VideoError> {
        let mut pb: videotoolbox::ffi::CVPixelBufferRef = std::ptr::null_mut();
        // SAFETY: surface is valid; pb is released below right after the
        // encode call.
        let status = unsafe {
            videotoolbox::ffi::CVPixelBufferCreateWithIOSurface(
                videotoolbox::ffi::kCFAllocatorDefault,
                surface.as_ptr().cast::<c_void>(),
                std::ptr::null(),
                &mut pb,
            )
        };
        if status != 0 || pb.is_null() {
            return Err(VideoError::CoreMedia(status));
        }
        let pts = apple_cf::cm::CMTime {
            value: pts_us,
            timescale: TIMESCALE_US,
            flags: 1,
            epoch: 0,
        };
        let props = force_keyframe.then(force_keyframe_properties);
        let props_ref = props
            .as_ref()
            .map_or(std::ptr::null(), |d| d.as_ptr().cast_const().cast());
        // SAFETY: session and pb are valid; props_ref borrows props, which
        // outlives the call.
        let status = unsafe {
            videotoolbox::ffi::VTCompressionSessionEncodeFrame(
                self.session,
                pb,
                pts,
                apple_cf::cm::CMTime::INVALID,
                props_ref,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        unsafe { videotoolbox::ffi::CFRelease(pb.cast()) };
        if status != 0 {
            return Err(VideoError::CoreMedia(status));
        }
        // SAFETY: session is valid; INVALID waits for all pending frames.
        let status = unsafe {
            videotoolbox::ffi::VTCompressionSessionCompleteFrames(
                self.session,
                apple_cf::cm::CMTime::INVALID,
            )
        };
        if status != 0 {
            return Err(VideoError::CoreMedia(status));
        }
        match self.out_rx.recv() {
            Ok(msg) => msg.map_err(VideoError::CoreMedia),
            // The sender lives in the callback state, which outlives the
            // session; a closed channel means the session is being torn down.
            Err(_) => Err(VideoError::CoreMedia(-1)),
        }
    }
}

impl Drop for EncoderSession {
    fn drop(&mut self) {
        // SAFETY: after invalidate no callback can fire anymore, so reclaiming
        // the leaked Arc is safe.
        unsafe {
            videotoolbox::ffi::VTCompressionSessionInvalidate(self.session);
            videotoolbox::ffi::CFRelease(self.session.cast());
            drop(Arc::from_raw(
                self.callback_state.cast::<EncoderCallbackState>(),
            ));
        }
    }
}

/// Builds `{ ForceKeyFrame: true }` as the frame-properties dictionary for
/// `VTCompressionSessionEncodeFrame`.
fn force_keyframe_properties() -> apple_cf::cf::CFDictionary {
    // SAFETY: both symbols are process-lifetime constant objects; retain/release is safe.
    let key_ptr = unsafe { cm::kVTEncodeFrameOptionKey_ForceKeyFrame } as *mut std::ffi::c_void;
    let val_ptr = unsafe { apple_cf::raw::kCFBooleanTrue as *mut std::ffi::c_void };
    let key =
        unsafe { apple_cf::cf::CFType::from_raw_retained(key_ptr) }.expect("force keyframe key");
    let value =
        unsafe { apple_cf::cf::CFType::from_raw_retained(val_ptr) }.expect("cf boolean true");
    apple_cf::cf::CFDictionary::from_pairs(&[(&key, &value)])
}

/// Encodes one frame, applying the frame-level ForceKeyFrame option when
/// requested. A failed force-keyframe attempt is logged and retried as a
/// regular frame, so the frame itself is never dropped because of the hint.
fn encode_maybe_forced<T, E: std::fmt::Display>(
    force_keyframe: bool,
    mut encode: impl FnMut(bool) -> Result<T, E>,
) -> Result<T, E> {
    if force_keyframe {
        match encode(true) {
            Ok(v) => return Ok(v),
            Err(e) => {
                tracing::warn!(err = %e, "force keyframe failed; retrying as regular frame");
            }
        }
    }
    encode(false)
}

pub struct VideoEncoder {
    session: Option<EncoderSession>,
    av1: Option<Av1Encoder>,
    codec: removent_proto::CodecId,
    width: usize,
    height: usize,
    param_sets: Option<Vec<Vec<u8>>>,
    force_keyframe_pending: bool,
}

impl VideoEncoder {
    pub fn new(
        codec: removent_proto::CodecId,
        width: usize,
        height: usize,
        bitrate_kbps: u32,
        fps: u8,
    ) -> Result<Self, VideoError> {
        let (session, av1) = if codec == removent_proto::CodecId::Av1 {
            (
                None,
                Some(Av1Encoder::new(width, height, bitrate_kbps, fps)?),
            )
        } else {
            (
                Some(EncoderSession::new(
                    vt_codec(codec),
                    width as i32,
                    height as i32,
                    bitrate_kbps,
                    fps,
                )?),
                None,
            )
        };
        Ok(Self {
            session,
            av1,
            codec,
            width,
            height,
            param_sets: None,
            force_keyframe_pending: false,
        })
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// Cached parameter sets (re-extracted from the format description on every
    /// keyframe; available after the first keyframe).
    pub fn parameter_sets(&self) -> Option<&[Vec<u8>]> {
        self.param_sets.as_deref()
    }

    pub fn request_keyframe(&mut self) {
        if let Some(av1) = self.av1.as_mut() {
            av1.request_keyframe();
        } else {
            self.force_keyframe_pending = true;
        }
    }

    /// Adjusts the target bitrate at runtime (used by adaptive rate control).
    pub fn set_bitrate_kbps(&mut self, kbps: u32) -> Result<(), VideoError> {
        if let Some(av1) = self.av1.as_mut() {
            return av1.set_bitrate_kbps(kbps);
        }
        self.session.as_ref().expect("VT session").set_i32_property(
            unsafe { videotoolbox::ffi::kVTCompressionPropertyKey_AverageBitRate },
            (kbps * 1000) as i32,
        )
    }

    /// Flushes delayed software AV1 packets at end of stream. VideoToolbox
    /// encoders are driven continuously and have no equivalent operation here.
    pub fn flush(&mut self) -> Result<Vec<EncodedVideoFrame>, VideoError> {
        if let Some(av1) = self.av1.as_mut() {
            return av1.flush();
        }
        Ok(Vec::new())
    }

    /// Encodes one BGRA frame. May return empty (no output when the encoder
    /// drops the frame).
    pub fn encode_bgra(
        &mut self,
        bgra: &[u8],
        pts_us: i64,
    ) -> Result<Vec<EncodedVideoFrame>, VideoError> {
        let bpr_expected = self.width * 4;
        if bgra.len() != bpr_expected * self.height {
            return Err(VideoError::PixelSizeMismatch {
                need: bpr_expected * self.height,
                got: bgra.len(),
            });
        }
        if let Some(av1) = self.av1.as_mut() {
            return av1.encode_bgra(bgra, pts_us);
        }
        let force_keyframe = std::mem::take(&mut self.force_keyframe_pending);

        let surface = IOSurface::create(self.width, self.height, BGRA_FOURCC, 4)
            .ok_or_else(|| VideoError::Surface("create failed".into()))?;
        {
            let mut guard = surface
                .lock_read_write()
                .map_err(|e| VideoError::Surface(format!("lock failed: {e}")))?;
            // SAFETY: base_address is valid while locked; sizes already checked.
            let dst = guard
                .base_address_mut()
                .ok_or_else(|| VideoError::Surface("no base address".into()))?;
            // SAFETY: both memory regions are owned by this function and lengths match.
            unsafe {
                std::ptr::copy_nonoverlapping(bgra.as_ptr(), dst, bgra.len());
            }
        }

        let Some(raw) = encode_maybe_forced(force_keyframe, |force| {
            self.session
                .as_ref()
                .expect("VT session")
                .encode(&surface, pts_us, force)
        })?
        else {
            return Ok(Vec::new());
        };

        if raw.data.is_empty() {
            return Ok(Vec::new());
        }

        let sb_ptr = raw.sample_buffer;
        let keyframe = is_sync_sample(sb_ptr);

        // Re-extract and cache parameter sets on every keyframe: when they
        // change, the keyframe must inline the current values
        // (protocol.md §6.1 config_changed semantics).
        if keyframe
            && !sb_ptr.is_null()
            && let Some(desc) = format_description_of_sample(sb_ptr)
        {
            let ps = extract_param_sets_from_desc(desc, is_hevc(self.codec));
            if !ps.is_empty() {
                tracing::debug!(count = ps.len(), "video parameter sets extracted");
                self.param_sets = Some(ps);
            }
        }

        let mut annexb = avcc_to_annexb(&raw.data)?;

        // Inline parameter sets into keyframes (protocol.md §6.1 config_changed semantics).
        if keyframe && let Some(ps) = &self.param_sets {
            let mut with_ps = Vec::with_capacity(annexb.len() + 64);
            for p in ps {
                with_ps.extend_from_slice(START_CODE);
                with_ps.extend_from_slice(p);
            }
            with_ps.extend_from_slice(&annexb);
            annexb = with_ps;
        }

        Ok(vec![EncodedVideoFrame {
            data: annexb,
            pts_us,
            keyframe,
        }])
    }
}

fn format_description_of_sample(sb: *mut std::ffi::c_void) -> Option<cm::CMFormatDescriptionRef> {
    // SAFETY: sb is a valid CMSampleBufferRef; the returned reference is owned
    // by the sample buffer.
    let desc = unsafe { videotoolbox::ffi::CMSampleBufferGetFormatDescription(sb.cast()) };
    if desc.is_null() {
        None
    } else {
        Some(desc.cast_mut().cast())
    }
}

fn is_sync_sample(sb: cm::CMSampleBufferRef) -> bool {
    // SAFETY: the symbol constant and API both come from system frameworks; sb is valid.
    unsafe {
        if sb.is_null() {
            // When undeterminable, treat as keyframe (inlining parameter sets is the safe direction).
            return true;
        }
        // VT records NotSync as a per-sample attachment; CMGetAttachment on
        // the sample buffer itself does not see it.
        let arr = cm::CMSampleBufferGetSampleAttachmentsArray(sb, 0);
        if !arr.is_null() && cm::CFArrayGetCount(arr) > 0 {
            let dict = cm::CFArrayGetValueAtIndex(arr, 0);
            if !dict.is_null() {
                let v = cm::CFDictionaryGetValue(dict, cm::kCMSampleAttachmentKey_NotSync);
                // NotSync missing or false ⇒ sync frame (keyframe).
                return v.is_null()
                    || !std::ptr::eq(v, videotoolbox::ffi::kCFBooleanTrue as *const c_void);
            }
        }
        // When the attachments array is missing, default to sync frame (keyframe).
        true
    }
}

fn extract_param_sets_from_desc(desc: cm::CMFormatDescriptionRef, hevc: bool) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut count: usize = 0;
    let mut hdr_len: i32 = 4;
    // SAFETY: desc is valid; the out pointers are used only within this call
    // and the data is owned by the description object — we copy it into Vecs
    // immediately.
    let get = if hevc {
        cm::CMVideoFormatDescriptionGetHEVCParameterSetAtIndex
    } else {
        cm::CMVideoFormatDescriptionGetH264ParameterSetAtIndex
    };
    unsafe {
        // Probe the total count with index=0 first.
        if get(
            desc,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut count,
            &mut hdr_len,
        ) != 0
        {
            return out;
        }
        for i in 0..count {
            let mut ptr: *const u8 = std::ptr::null();
            let mut size: usize = 0;
            let mut cnt: usize = 0;
            if get(desc, i, &mut ptr, &mut size, &mut cnt, &mut hdr_len) == 0 && !ptr.is_null() {
                out.push(std::slice::from_raw_parts(ptr, size).to_vec());
            }
        }
    }
    out
}

// ---------------- Decoding ----------------

pub struct VideoDecoder {
    /// Holds a +1 reference; released automatically on Drop.
    _format_desc: Option<apple_cf::cm::CMFormatDescription>,
    session: Option<CompatibleDecompressionSession>,
    /// Wrapped in a Mutex because `mpsc::Receiver` is `!Sync` while the
    /// decoder must be `Sync` for shared access across tasks; usage is
    /// effectively single-task, so contention is nil.
    rx: std::sync::Mutex<mpsc::Receiver<DecodedBgra>>,
    av1: std::sync::Mutex<Option<Av1Decoder>>,
    av1_tx: Option<mpsc::Sender<DecodedBgra>>,
    hevc: bool,
    width: usize,
    height: usize,
}

pub struct DecodedBgra {
    pub data: Vec<u8>,
    pub pts_us: i64,
}

impl VideoDecoder {
    pub fn new(
        codec: removent_proto::CodecId,
        width: usize,
        height: usize,
        param_sets: &[Vec<u8>],
    ) -> Result<Self, VideoError> {
        if codec != removent_proto::CodecId::Av1 && param_sets.is_empty() {
            return Err(VideoError::NoParameterSets);
        }
        let (tx, rx) = mpsc::channel::<DecodedBgra>();
        if codec == removent_proto::CodecId::Av1 {
            return Ok(Self {
                _format_desc: None,
                session: None,
                rx: std::sync::Mutex::new(rx),
                av1: std::sync::Mutex::new(Some(Av1Decoder::new(width, height)?)),
                av1_tx: Some(tx),
                hevc: false,
                width,
                height,
            });
        }
        let hevc = is_hevc(codec);
        let ptrs: Vec<*const u8> = param_sets.iter().map(|p| p.as_ptr()).collect();
        let sizes: Vec<usize> = param_sets.iter().map(|p| p.len()).collect();
        let mut desc: cm::CMFormatDescriptionRef = std::ptr::null_mut();
        let status = unsafe {
            if hevc {
                cm::CMVideoFormatDescriptionCreateFromHEVCParameterSets(
                    cm::default_allocator(),
                    ptrs.len(),
                    ptrs.as_ptr(),
                    sizes.as_ptr(),
                    4,
                    std::ptr::null(),
                    &mut desc,
                )
            } else {
                cm::CMVideoFormatDescriptionCreateFromH264ParameterSets(
                    cm::default_allocator(),
                    ptrs.len(),
                    ptrs.as_ptr(),
                    sizes.as_ptr(),
                    4,
                    &mut desc,
                )
            }
        };
        if status != 0 {
            return Err(VideoError::CoreMedia(status));
        }

        let format_desc = apple_cf::cm::CMFormatDescription::from_raw(desc.cast())
            .expect("non-null format description");
        let session = CompatibleDecompressionSession::new(&format_desc, move |frame| {
            let Some(pb) = frame.image_buffer else { return };
            if frame.status != 0 {
                tracing::warn!(status = frame.status, "decode callback error");
                return;
            }
            // presentation_time is (value, timescale); normalized to microseconds.
            let pts_us = scale_to_us(frame.presentation_time);
            if let Some(bgra) = pixel_buffer_to_bgra(&pb) {
                let _ = tx.send(DecodedBgra { data: bgra, pts_us });
            }
        })?;
        Ok(Self {
            _format_desc: Some(format_desc),
            session: Some(session),
            rx: std::sync::Mutex::new(rx),
            av1: std::sync::Mutex::new(None),
            av1_tx: None,
            hevc,
            width,
            height,
        })
    }

    pub fn try_recv_decoded(&self) -> Option<DecodedBgra> {
        self.rx.lock().ok()?.try_recv().ok()
    }

    /// Flushes delayed software AV1 pictures at end of stream. VideoToolbox
    /// decoders deliver frames through their callback and need no explicit
    /// flush here.
    pub fn flush(&self) -> Result<(), VideoError> {
        let mut av1 = self
            .av1
            .lock()
            .map_err(|_| VideoError::Av1("decoder lock poisoned".into()))?;
        let Some(decoder) = av1.as_mut() else {
            return Ok(());
        };
        let frames = decoder.flush()?;
        let tx = self.av1_tx.as_ref().expect("AV1 output sender");
        for frame in frames {
            let _ = tx.send(frame);
        }
        Ok(())
    }

    /// Decodes one encoded frame. H264/HEVC input is Annex-B; AV1 input is a
    /// complete temporal-unit OBU payload. Fetch output with
    /// [`Self::try_recv_decoded`].
    pub fn decode_annexb(&self, annexb: &[u8], pts_us: i64) -> Result<(), VideoError> {
        if let Some(decoder) = self
            .av1
            .lock()
            .map_err(|_| VideoError::Av1("decoder lock poisoned".into()))?
            .as_mut()
        {
            let frames = decoder.decode(annexb, pts_us)?;
            let tx = self.av1_tx.as_ref().expect("AV1 output sender").clone();
            for frame in frames {
                let _ = tx.send(frame);
            }
            return Ok(());
        }
        // Split as Annex-B first, filter out parameter sets, then rebuild as AVCC.
        let nals: Vec<&[u8]> = split_annexb_nals(annexb)
            .into_iter()
            .filter(|n| !is_param_set(nal_type(n, self.hevc), self.hevc))
            .collect();
        if nals.is_empty() {
            tracing::debug!("decode_annexb: nothing to decode after filtering");
            return Ok(());
        }
        let mut payload = Vec::with_capacity(annexb.len());
        for n in &nals {
            payload.extend_from_slice(&(n.len() as u32).to_be_bytes());
            payload.extend_from_slice(n);
        }

        // SAFETY: the following FFI combination follows CoreMedia Create/Copy rules:
        // BlockBuffer(+1) → SampleBuffer retains internally → we release the
        // BlockBuffer and our own SB reference.
        unsafe {
            let mut bb: cm::CMBlockBufferRef = std::ptr::null_mut();
            let st = cm::CMBlockBufferCreateWithMemoryBlock(
                cm::default_allocator(),
                std::ptr::null_mut(),
                payload.len(),
                cm::default_allocator(),
                std::ptr::null(),
                0,
                payload.len(),
                0,
                &mut bb,
            );
            if st != 0 {
                return Err(VideoError::CoreMedia(st));
            }
            let st =
                cm::CMBlockBufferReplaceDataBytes(payload.as_ptr().cast(), bb, 0, payload.len());
            if st != 0 {
                videotoolbox::ffi::CFRelease(bb.cast());
                return Err(VideoError::CoreMedia(st));
            }
            let timing = [cm::CMSampleTimingInfo::from_us(pts_us)];
            let size = [payload.len()];
            let mut sb: cm::CMSampleBufferRef = std::ptr::null_mut();
            let st = cm::CMSampleBufferCreateReady(
                cm::default_allocator(),
                bb,
                self._format_desc
                    .as_ref()
                    .map(|d| d.as_ptr())
                    .unwrap_or(std::ptr::null_mut()),
                1,
                1,
                timing.as_ptr(),
                1,
                size.as_ptr(),
                &mut sb,
            );
            videotoolbox::ffi::CFRelease(bb.cast());
            if st != 0 {
                return Err(VideoError::CoreMedia(st));
            }
            if sb.is_null() {
                return Err(VideoError::NullSampleBuffer);
            }
            let wrapped = apple_cf::cm::CMSampleBuffer::from_raw_retained(sb)
                .ok_or(VideoError::NullSampleBuffer)?;
            videotoolbox::ffi::CFRelease(sb.cast()); // return our temporary +1
            self.session
                .as_ref()
                .expect("VT decoder session")
                .decode(&wrapped)?;
        }
        Ok(())
    }

    pub fn dimensions(&self) -> (usize, usize) {
        (self.width, self.height)
    }
}

fn scale_to_us((value, timescale): (i64, i32)) -> i64 {
    if timescale == TIMESCALE_US {
        value
    } else if timescale == 0 {
        0
    } else {
        value * TIMESCALE_US as i64 / timescale as i64
    }
}

/// CVPixelBuffer → BGRA (8/10-bit bi-planar YUV conversion for non-BGRA formats).
fn pixel_buffer_to_bgra(pb: &apple_cf::cv::CVPixelBuffer) -> Option<Vec<u8>> {
    let w = pb.width();
    let h = pb.height();
    let fmt = pb.pixel_format();
    let out_len = w.checked_mul(h)?.checked_mul(4)?;
    let guard = pb.lock_read_only().ok()?;

    if fmt == BGRA_FOURCC {
        let base = guard.base_address();
        let bpr = pb.bytes_per_row();
        let mut out = vec![0u8; out_len];
        for row in 0..h {
            // SAFETY: row ranges are protected by the read lock; bpr >= w*4 is
            // guaranteed by the pixel format.
            let src = unsafe { base.add(row * bpr) };
            let dst = &mut out[row * w * 4..(row + 1) * w * 4];
            dst.copy_from_slice(unsafe { std::slice::from_raw_parts(src, w * 4) });
        }
        return Some(out);
    }

    if fmt == u32::from_be_bytes(*b"ARGB") {
        // ARGB memory order [A,R,G,B] must be reordered to BGRA [B,G,R,A]
        // (i.e. reverse each 4-byte pixel).
        let base = guard.base_address();
        let bpr = pb.bytes_per_row();
        let mut out = vec![0u8; out_len];
        for row in 0..h {
            // SAFETY: row ranges are protected by the read lock; bpr >= w*4 is
            // guaranteed by the pixel format.
            let src = unsafe { std::slice::from_raw_parts(base.add(row * bpr), w * 4) };
            let dst = &mut out[row * w * 4..(row + 1) * w * 4];
            let (src_px, _) = src.as_chunks::<4>();
            let (dst_px, _) = dst.as_chunks_mut::<4>();
            for (s, d) in src_px.iter().zip(dst_px.iter_mut()) {
                *d = [s[3], s[2], s[1], s[0]]; // [B,G,R,A]
            }
        }
        return Some(out);
    }

    if matches!(fmt, f if f == u32::from_be_bytes(*b"420v") || f == u32::from_be_bytes(*b"420f")) {
        // Bi-planar NV12: use the plane APIs to get each base address and
        // stride, avoiding layout guessing.
        let y_ptr = guard.base_address_of_plane(0)?;
        let uv_ptr = guard.base_address_of_plane(1)?;
        let y_stride = pb.bytes_per_row_of_plane(0);
        let uv_stride = pb.bytes_per_row_of_plane(1);
        let bt709 = is_bt709(pb);
        let color = YuvColor {
            bt709,
            full_range: fmt == u32::from_be_bytes(*b"420f"),
        };
        let out = nv12_to_bgra(y_ptr, y_stride, uv_ptr, uv_stride, w, h, color);
        // Unlock only after the planes have been fully read.
        drop(guard);
        return Some(out);
    }

    if fmt == u32::from_be_bytes(*b"x420") {
        // 10-bit video-range 4:2:0 (P010): each little-endian component is
        // stored in the ten most-significant bits of a 16-bit word.
        let y_ptr = guard.base_address_of_plane(0)?;
        let uv_ptr = guard.base_address_of_plane(1)?;
        let y_stride = pb.bytes_per_row_of_plane(0);
        let uv_stride = pb.bytes_per_row_of_plane(1);
        let out = p010_video_to_bgra(y_ptr, y_stride, uv_ptr, uv_stride, w, h, is_bt709(pb));
        // Unlock only after the planes have been fully read.
        drop(guard);
        return Some(out);
    }
    tracing::warn!(
        fmt = format_args!("{fmt:#010x}"),
        "unexpected decode output format"
    );
    None
}

/// Reads the color-matrix attachment of the image buffer; returns true for
/// BT.709, treats missing/other as BT.601.
fn is_bt709(pb: &apple_cf::cv::CVPixelBuffer) -> bool {
    // SAFETY: pb is valid; the attachment is a +0 borrowed reference and
    // from_raw_retained retains it itself.
    unsafe {
        let key = std::ptr::addr_of!(apple_cf::raw::kCVImageBufferYCbCrMatrixKey).read();
        let v = apple_cf::raw::CVBufferGetAttachment(pb.as_ptr().cast(), key, std::ptr::null_mut());
        if v.is_null() {
            return false;
        }
        apple_cf::cf::CFString::from_raw_retained(v.cast_mut())
            .is_some_and(|s| s.to_string_lossy() == "ITU_R_709-2")
    }
}

/// BT.601/BT.709 NV12 → BGRA. Valid uv bytes per row = ceil(w/2)*2.
#[derive(Clone, Copy)]
struct YuvColor {
    bt709: bool,
    full_range: bool,
}

fn nv12_to_bgra(
    y_plane: *const u8,
    y_stride: usize,
    uv_plane: *const u8,
    uv_stride: usize,
    w: usize,
    h: usize,
    color: YuvColor,
) -> Vec<u8> {
    // Fixed-point 8.8 coefficients; BT.601: Kr=0.299 Kb=0.114,
    // BT.709: Kr=0.2126 Kb=0.0722.
    let (y_scale, y_offset, cr, cgu, cgv, cb) = match (color.bt709, color.full_range) {
        (false, false) => (298, 16, 409, 100, 208, 516),
        (true, false) => (298, 16, 459, 55, 136, 541),
        (false, true) => (256, 0, 359, 88, 183, 454),
        (true, true) => (256, 0, 403, 48, 120, 475),
    };
    let mut out = vec![0u8; w * h * 4];
    let uv_w = w.div_ceil(2) * 2;
    for row in 0..h {
        // SAFETY: all row offsets stay within the corresponding plane
        // allocation (stride*h / stride*(h/2)).
        let y_row = unsafe { std::slice::from_raw_parts(y_plane.add(row * y_stride), w) };
        let uv_row =
            unsafe { std::slice::from_raw_parts(uv_plane.add((row / 2) * uv_stride), uv_w) };
        for (col, &y_raw) in y_row.iter().enumerate() {
            let y = i32::from(y_raw);
            let uv_i = (col / 2) * 2;
            let u = i32::from(uv_row[uv_i]) - 128;
            let v = i32::from(uv_row[uv_i + 1]) - 128;
            let c = y - y_offset;
            let r = ((y_scale * c + cr * v + 128) >> 8).clamp(0, 255);
            let g = ((y_scale * c - cgu * u - cgv * v + 128) >> 8).clamp(0, 255);
            let b = ((y_scale * c + cb * u + 128) >> 8).clamp(0, 255);
            let o = (row * w + col) * 4;
            out[o] = b as u8;
            out[o + 1] = g as u8;
            out[o + 2] = r as u8;
            out[o + 3] = 255;
        }
    }
    out
}

/// BT.601/BT.709 limited-range P010 (`x420`) → BGRA.
fn p010_video_to_bgra(
    y_plane: *const u8,
    y_stride: usize,
    uv_plane: *const u8,
    uv_stride: usize,
    w: usize,
    h: usize,
    bt709: bool,
) -> Vec<u8> {
    let (cr, cgu, cgv, cb) = if bt709 {
        (459, 55, 136, 541)
    } else {
        (409, 100, 208, 516)
    };
    let mut out = vec![0u8; w * h * 4];
    for row in 0..h {
        for col in 0..w {
            let y_offset = row * y_stride + col * 2;
            let uv_offset = (row / 2) * uv_stride + (col / 2) * 4;
            // SAFETY: CoreVideo guarantees that each plane contains its
            // stride-sized rows. `read_unaligned` avoids assuming word
            // alignment for a plane base or padded row.
            let y = i32::from(unsafe { read_p010(y_plane.add(y_offset)) });
            let u = i32::from(unsafe { read_p010(uv_plane.add(uv_offset)) }) - 512;
            let v = i32::from(unsafe { read_p010(uv_plane.add(uv_offset + 2)) }) - 512;
            let c = y - 64;
            // The 10-bit ranges are exactly four times their 8-bit
            // counterparts, so 8.8 coefficients use a 10-bit final shift.
            let r = ((298 * c + cr * v + 512) >> 10).clamp(0, 255);
            let g = ((298 * c - cgu * u - cgv * v + 512) >> 10).clamp(0, 255);
            let b = ((298 * c + cb * u + 512) >> 10).clamp(0, 255);
            let o = (row * w + col) * 4;
            out[o] = b as u8;
            out[o + 1] = g as u8;
            out[o + 2] = r as u8;
            out[o + 3] = 255;
        }
    }
    out
}

/// Reads one little-endian P010 word and removes its six padding bits.
unsafe fn read_p010(ptr: *const u8) -> u16 {
    u16::from_le(unsafe { ptr.cast::<u16>().read_unaligned() }) >> 6
}

// SAFETY: CoreFoundation/CoreMedia objects are thread-safe (CFType docs);
// the mpsc receiver is protected by a Mutex, so shared (&) access is safe.
// VideoDecoder only holds these members.
unsafe impl Send for VideoDecoder {}
unsafe impl Sync for VideoDecoder {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A force-keyframe attempt that fails must be retried as a regular frame:
    /// the frame is encoded, not dropped.
    #[test]
    fn force_keyframe_failure_retries_without_hint() {
        let calls = RefCell::new(Vec::new());
        let result: Result<i32, VideoError> = encode_maybe_forced(true, |force| {
            calls.borrow_mut().push(force);
            if force {
                Err(VideoError::CoreMedia(-12902)) // kVTPropertyNotSupportedErr
            } else {
                Ok(1)
            }
        });
        assert_eq!(result.unwrap(), 1);
        assert_eq!(*calls.borrow(), vec![true, false]);
    }

    /// When the regular retry also fails, the error propagates.
    #[test]
    fn force_keyframe_failure_propagates_when_retry_fails() {
        let result: Result<(), VideoError> =
            encode_maybe_forced(true, |_| Err(VideoError::CoreMedia(-1)));
        assert!(matches!(result, Err(VideoError::CoreMedia(-1))));
    }

    /// Without a pending keyframe request the hint is never attached.
    #[test]
    fn no_force_keyframe_single_plain_encode() {
        let calls = RefCell::new(Vec::new());
        let result: Result<i32, VideoError> = encode_maybe_forced(false, |force| {
            calls.borrow_mut().push(force);
            Ok(2)
        });
        assert_eq!(result.unwrap(), 2);
        assert_eq!(*calls.borrow(), vec![false]);
    }

    #[test]
    fn nv12_video_range_maps_neutral_black_and_white() {
        let y = [16, 235];
        let uv = [128, 128];
        let bgra = nv12_to_bgra(
            y.as_ptr(),
            2,
            uv.as_ptr(),
            2,
            2,
            1,
            YuvColor {
                bt709: true,
                full_range: false,
            },
        );
        assert_eq!(bgra, [0, 0, 0, 255, 255, 255, 255, 255]);
    }

    #[test]
    fn nv12_full_range_maps_neutral_black_and_white() {
        let y = [0, 255];
        let uv = [128, 128];
        let bgra = nv12_to_bgra(
            y.as_ptr(),
            2,
            uv.as_ptr(),
            2,
            2,
            1,
            YuvColor {
                bt709: true,
                full_range: true,
            },
        );
        assert_eq!(bgra, [0, 0, 0, 255, 255, 255, 255, 255]);
    }

    #[test]
    fn p010_video_range_maps_neutral_black_and_white_with_padding() {
        let mut y = Vec::new();
        for samples in [[64_u16, 940], [940, 64]] {
            for sample in samples {
                y.extend_from_slice(&(sample << 6).to_le_bytes());
            }
            y.extend_from_slice(&[0, 0]);
        }
        let mut uv = Vec::new();
        for sample in [512_u16, 512] {
            uv.extend_from_slice(&(sample << 6).to_le_bytes());
        }
        uv.extend_from_slice(&[0, 0]);

        let bgra = p010_video_to_bgra(y.as_ptr(), 6, uv.as_ptr(), 6, 2, 2, true);
        assert_eq!(
            bgra,
            [
                0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 255,
            ]
        );
    }
}
