use super::*;

pub(super) fn format_description_of_sample(
    sb: *mut std::ffi::c_void,
) -> Option<cm::CMFormatDescriptionRef> {
    // SAFETY: sb is a valid CMSampleBufferRef; the returned reference is owned
    // by the sample buffer.
    let desc = unsafe { videotoolbox::ffi::CMSampleBufferGetFormatDescription(sb.cast()) };
    if desc.is_null() {
        None
    } else {
        Some(desc.cast_mut().cast())
    }
}

pub(super) fn is_sync_sample(sb: cm::CMSampleBufferRef) -> bool {
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

pub(super) fn extract_param_sets_from_desc(
    desc: cm::CMFormatDescriptionRef,
    hevc: bool,
) -> Vec<Vec<u8>> {
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
    pub(super) _format_desc: Option<apple_cf::cm::CMFormatDescription>,
    pub(super) session: Option<CompatibleDecompressionSession>,
    /// Wrapped in a Mutex because `mpsc::Receiver` is `!Sync` while the
    /// decoder must be `Sync` for shared access across tasks; usage is
    /// effectively single-task, so contention is nil.
    pub(super) rx: std::sync::Mutex<mpsc::Receiver<DecodedBgra>>,
    pub(super) av1: std::sync::Mutex<Option<Av1Decoder>>,
    pub(super) output: Arc<dyn Fn(DecodedBgra) + Send + Sync>,
    pub(super) hevc: bool,
    pub(super) width: usize,
    pub(super) height: usize,
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
        let (tx, rx) = mpsc::channel();
        let mut decoder = Self::with_output(codec, width, height, param_sets, move |frame| {
            let _ = tx.send(frame);
        })?;
        decoder.rx = std::sync::Mutex::new(rx);
        Ok(decoder)
    }

    /// Live playback output. The callback must not block and should replace
    /// unread frames; this bypasses the lossless queue used by offline callers.
    pub fn with_output(
        codec: removent_proto::CodecId,
        width: usize,
        height: usize,
        param_sets: &[Vec<u8>],
        output: impl Fn(DecodedBgra) + Send + Sync + 'static,
    ) -> Result<Self, VideoError> {
        if codec != removent_proto::CodecId::Av1 && param_sets.is_empty() {
            return Err(VideoError::NoParameterSets);
        }
        let (_, rx) = mpsc::channel();
        let output: Arc<dyn Fn(DecodedBgra) + Send + Sync> = Arc::new(output);
        if codec == removent_proto::CodecId::Av1 {
            return Ok(Self {
                _format_desc: None,
                session: None,
                rx: std::sync::Mutex::new(rx),
                av1: std::sync::Mutex::new(Some(Av1Decoder::new(width, height)?)),
                output,
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
        let callback = output.clone();
        let session = CompatibleDecompressionSession::new(&format_desc, move |frame| {
            let Some(pb) = frame.image_buffer else { return };
            if frame.status != 0 {
                tracing::warn!(status = frame.status, "decode callback error");
                return;
            }
            // presentation_time is (value, timescale); normalized to microseconds.
            let pts_us = scale_to_us(frame.presentation_time);
            if let Some(bgra) = pixel_buffer_to_bgra(&pb) {
                callback(DecodedBgra { data: bgra, pts_us });
            }
        })?;
        Ok(Self {
            _format_desc: Some(format_desc),
            session: Some(session),
            rx: std::sync::Mutex::new(rx),
            av1: std::sync::Mutex::new(None),
            output,
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
        for frame in frames {
            (self.output)(frame);
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
            for frame in frames {
                (self.output)(frame);
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

pub(super) fn scale_to_us((value, timescale): (i64, i32)) -> i64 {
    if timescale == TIMESCALE_US {
        value
    } else if timescale == 0 {
        0
    } else {
        value * TIMESCALE_US as i64 / timescale as i64
    }
}
