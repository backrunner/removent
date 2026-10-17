use super::*;
use videotoolbox::ffi as vt;

pub(super) fn create(
    codec: Codec,
    width: i32,
    height: i32,
    state: *mut c_void,
) -> (vt::VTCompressionSessionRef, i32) {
    // SAFETY: these framework constants have process lifetime; the dictionary
    // retains its entries for both creation attempts.
    let hardware = unsafe {
        apple_cf::cf::CFType::from_raw_borrowed(
            vt::kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder
                .cast_mut()
                .cast(),
        )
    }
    .unwrap();
    let enabled =
        unsafe { apple_cf::cf::CFType::from_raw_borrowed(vt::kCFBooleanTrue.cast_mut().cast()) }
            .unwrap();
    let specification = apple_cf::cf::CFDictionary::from_pairs(&[(&hardware, &enabled)]);
    let mut status = 0;
    for spec in [specification.as_ptr().cast_const().cast(), std::ptr::null()] {
        let mut session = std::ptr::null_mut();
        // SAFETY: the caller retains state until the session is invalidated.
        status = unsafe {
            vt::VTCompressionSessionCreate(
                vt::kCFAllocatorDefault,
                width,
                height,
                codec.as_cm_codec_type(),
                spec,
                std::ptr::null(),
                vt::kCFAllocatorDefault,
                Some(encode_output_callback),
                state,
                &mut session,
            )
        };
        if status == 0 && !session.is_null() {
            return (session, status);
        }
        if !session.is_null() {
            unsafe {
                vt::VTCompressionSessionInvalidate(session);
                vt::CFRelease(session.cast());
            }
        }
        if !spec.is_null() {
            tracing::warn!(
                status,
                width,
                height,
                "hardware encoder unavailable; trying compatible encoder"
            );
        }
    }
    (std::ptr::null_mut(), status)
}

pub(super) fn is_hardware(session: vt::VTCompressionSessionRef) -> bool {
    let mut value: *const c_void = std::ptr::null();
    // SAFETY: session is live; CopyProperty returns an owned CFBoolean.
    let status = unsafe {
        vt::VTSessionCopyProperty(
            session,
            vt::kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder,
            vt::kCFAllocatorDefault,
            std::ptr::from_mut(&mut value).cast(),
        )
    };
    if status != 0 || value.is_null() {
        return false;
    }
    let enabled = std::ptr::eq(value, unsafe { vt::kCFBooleanTrue.cast() });
    unsafe {
        vt::CFRelease(value);
    }
    enabled
}
