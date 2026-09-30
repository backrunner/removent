use super::*;

// ---------------- Encoding ----------------

/// One encoded sample straight from the VT output callback: the AVCC payload
/// plus the retained sample buffer (needed for keyframe and parameter-set
/// inspection). The sample buffer reference is released on drop.
pub(super) struct RawEncodedSample {
    pub(super) data: Vec<u8>,
    pub(super) sample_buffer: cm::CMSampleBufferRef,
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

pub(super) struct EncoderCallbackState {
    pub(super) out_tx: mpsc::Sender<Result<Option<RawEncodedSample>, i32>>,
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
pub(super) struct EncoderSession {
    pub(super) session: videotoolbox::ffi::VTCompressionSessionRef,
    /// Leaked `Arc<EncoderCallbackState>` handed to the callback; reclaimed on drop.
    pub(super) callback_state: *mut c_void,
    pub(super) out_rx: mpsc::Receiver<Result<Option<RawEncodedSample>, i32>>,
    pub(super) quality_ceiling_supported: bool,
}

// SAFETY: VideoToolbox sessions are documented as thread-safe; the output
// callback only touches the channel sender.
unsafe impl Send for EncoderSession {}

impl EncoderSession {
    pub(super) fn new(
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
        let mut s = Self {
            session,
            callback_state,
            out_rx: rx,
            quality_ceiling_supported: false,
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
        // Apple allows the encoder to drop frames to satisfy this quality
        // bound. Older hardware/modes may not implement the optional property.
        match s.set_i32_property(
            unsafe { cm::kVTCompressionPropertyKey_MaxAllowedFrameQP.cast() },
            SCREEN_MAX_FRAME_QP,
        ) {
            Ok(()) => s.quality_ceiling_supported = true,
            Err(error) => {
                tracing::warn!(%error, "encoder has no frame QP ceiling; using frame-budget protection")
            }
        }
        // SAFETY: session is valid.
        let status =
            unsafe { videotoolbox::ffi::VTCompressionSessionPrepareToEncodeFrames(session) };
        if status != 0 {
            return Err(VideoError::CoreMedia(status));
        }
        Ok(s)
    }

    pub(super) fn set_bool_property(
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

    pub(super) fn set_i32_property(
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

    pub(super) fn set_f64_property(
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
    pub(super) fn encode(
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
pub(super) fn force_keyframe_properties() -> apple_cf::cf::CFDictionary {
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
pub(super) fn encode_maybe_forced<T, E: std::fmt::Display>(
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
    pub(super) session: Option<EncoderSession>,
    pub(super) av1: Option<Av1Encoder>,
    pub(super) codec: removent_proto::CodecId,
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) param_sets: Option<Vec<Vec<u8>>>,
    pub(super) force_keyframe_pending: bool,
    pub(super) surface: Option<IOSurface>,
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
            surface: None,
        })
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// Whether this codec accepted its quantization quality limit.
    pub fn quality_ceiling_supported(&self) -> bool {
        self.av1.is_some()
            || self
                .session
                .as_ref()
                .is_some_and(|s| s.quality_ceiling_supported)
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

        // encode waits for CompleteFrames, so the surface can be reused once
        // each call returns instead of allocating a new IOSurface every frame.
        if self.surface.is_none() {
            // VideoToolbox can import the surface as a Metal texture. An even
            // width alone does not guarantee its required 16-byte row alignment
            // (318 BGRA pixels occupy 1272 bytes). The default IOSurface allocator
            // does not add padding on every driver, so request it explicitly.
            let stride = bpr_expected
                .checked_next_multiple_of(64)
                .ok_or_else(|| VideoError::Surface("row stride overflow".into()))?;
            let alloc_size = stride
                .checked_mul(self.height)
                .ok_or_else(|| VideoError::Surface("allocation size overflow".into()))?;
            self.surface = Some(
                IOSurface::create_with_properties(
                    self.width,
                    self.height,
                    BGRA_FOURCC,
                    4,
                    stride,
                    alloc_size,
                    None,
                )
                .ok_or_else(|| VideoError::Surface("create failed".into()))?,
            );
        }
        let surface = self.surface.as_ref().expect("encoder surface");
        {
            let mut guard = surface
                .lock_read_write()
                .map_err(|e| VideoError::Surface(format!("lock failed: {e}")))?;
            let stride = guard.bytes_per_row();
            if stride < bpr_expected {
                return Err(VideoError::Surface("row stride smaller than input".into()));
            }
            // SAFETY: base_address is valid while locked; sizes already checked.
            let dst = guard
                .base_address_mut()
                .ok_or_else(|| VideoError::Surface("no base address".into()))?;
            // SAFETY: both memory regions are owned by this function and lengths match.
            unsafe {
                for row in 0..self.height {
                    std::ptr::copy_nonoverlapping(
                        bgra.as_ptr().add(row * bpr_expected),
                        dst.add(row * stride),
                        bpr_expected,
                    );
                }
            }
        }

        let encoded = encode_maybe_forced(force_keyframe, |force| {
            self.session
                .as_ref()
                .expect("VT session")
                .encode(surface, pts_us, force)
        });
        if encoded.is_err() {
            // An errored completion may still retain this surface in VT.
            self.surface = None;
        }
        let Some(raw) = encoded? else {
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
