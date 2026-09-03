//! VideoToolbox decompression wrapper compatible with the project's macOS 13 target.
//!
//! The upstream wrapper's basic `decode` method currently calls
//! `VTDecompressionSessionDecodeFrameWithOptions`, which is absent from older
//! SDKs. This local wrapper uses the original decode API available throughout
//! Removent's supported macOS range.

use core::ffi::c_void;
use std::sync::{Arc, Mutex};
use videotoolbox::{VTError, decompression::DecodedFrame, ffi};

type DecodeCallback = Box<dyn FnMut(DecodedFrame) + Send + 'static>;

struct CallbackState {
    callback: Mutex<DecodeCallback>,
}

pub(crate) struct CompatibleDecompressionSession {
    session: ffi::VTDecompressionSessionRef,
    // Keeps the callback context alive until after the session is invalidated.
    _state: Arc<CallbackState>,
}

unsafe impl Send for CompatibleDecompressionSession {}
unsafe impl Sync for CompatibleDecompressionSession {}

impl CompatibleDecompressionSession {
    pub(crate) fn new<F>(
        format_description: &apple_cf::cm::CMFormatDescription,
        callback: F,
    ) -> Result<Self, VTError>
    where
        F: FnMut(DecodedFrame) + Send + 'static,
    {
        let state = Arc::new(CallbackState {
            callback: Mutex::new(Box::new(callback)),
        });
        let record = ffi::VTDecompressionOutputCallbackRecord {
            decompression_output_callback: decode_trampoline,
            decompression_output_ref_con: Arc::as_ptr(&state).cast::<c_void>().cast_mut(),
        };
        let mut session = std::ptr::null_mut();
        let status = unsafe {
            ffi::VTDecompressionSessionCreate(
                ffi::kCFAllocatorDefault,
                format_description.as_ptr().cast(),
                std::ptr::null(),
                std::ptr::null(),
                &record,
                &mut session,
            )
        };
        if status != 0 || session.is_null() {
            return Err(VTError::EncoderCallback(status));
        }
        Ok(Self {
            session,
            _state: state,
        })
    }

    pub(crate) fn decode(
        &self,
        sample_buffer: &apple_cf::cm::CMSampleBuffer,
    ) -> Result<(), VTError> {
        let mut info_flags = 0;
        let status = unsafe {
            ffi::VTDecompressionSessionDecodeFrame(
                self.session,
                sample_buffer.as_ptr().cast(),
                0,
                std::ptr::null_mut(),
                &mut info_flags,
            )
        };
        if status == 0 {
            Ok(())
        } else {
            Err(VTError::EncoderCallback(status))
        }
    }
}

impl Drop for CompatibleDecompressionSession {
    fn drop(&mut self) {
        if !self.session.is_null() {
            unsafe {
                // Keep `_state` alive until all callbacks have returned.
                let _ = ffi::VTDecompressionSessionWaitForAsynchronousFrames(self.session);
                ffi::VTDecompressionSessionInvalidate(self.session);
                ffi::CFRelease(self.session.cast());
            }
        }
    }
}

unsafe extern "C" fn decode_trampoline(
    output_ref_con: *mut c_void,
    _source_frame_ref_con: *mut c_void,
    status: ffi::OSStatus,
    info_flags: ffi::VTDecodeInfoFlags,
    image_buffer: *mut c_void,
    pts: ffi::CMTime,
    duration: ffi::CMTime,
) {
    // SAFETY: `output_ref_con` points to `_state`, which remains alive until
    // all asynchronous callbacks finish during session teardown.
    let Some(state) = (unsafe { output_ref_con.cast::<CallbackState>().as_ref() }) else {
        return;
    };
    let image = if image_buffer.is_null() {
        None
    } else {
        // SAFETY: VideoToolbox supplies a valid image buffer for the duration
        // of this callback. Retaining lets the Rust wrapper outlive the call.
        unsafe { ffi::CFRetain(image_buffer.cast_const()) };
        apple_cf::cv::CVPixelBuffer::from_raw(image_buffer)
    };
    let frame = DecodedFrame {
        image_buffer: image,
        presentation_time: (pts.value, pts.timescale),
        duration: (duration.value, duration.timescale),
        info_flags,
        status,
    };
    let Ok(mut callback) = state.callback.lock() else {
        return;
    };
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(frame)));
}
