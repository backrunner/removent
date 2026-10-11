//! ABI layout assertions for the hand-written `#[repr(C)]` value types shared
//! with Apple's frameworks and the Swift bridge.
//!
//! These structs are passed by value (or read out of framework-populated
//! buffers) across the FFI boundary. If their size or alignment ever drifts
//! from what the C / Swift side expects, the data marshalling silently
//! corrupts. These tests pin the layout so accidental field reordering / type
//! changes are caught at `cargo test` time rather than as runtime garbage.

use std::mem::{align_of, size_of};

#[cfg(feature = "cg")]
#[test]
fn cg_value_type_layout() {
    use apple_cf::cg::{CGAffineTransform, CGPoint, CGRect, CGSize, CGVector};

    assert_eq!(size_of::<CGPoint>(), 16, "CGPoint size drifted");
    assert_eq!(align_of::<CGPoint>(), 8, "CGPoint alignment drifted");

    assert_eq!(size_of::<CGSize>(), 16, "CGSize size drifted");
    assert_eq!(align_of::<CGSize>(), 8, "CGSize alignment drifted");

    assert_eq!(size_of::<CGRect>(), 32, "CGRect size drifted");
    assert_eq!(align_of::<CGRect>(), 8, "CGRect alignment drifted");

    assert_eq!(size_of::<CGVector>(), 16, "CGVector size drifted");
    assert_eq!(align_of::<CGVector>(), 8, "CGVector alignment drifted");

    assert_eq!(
        size_of::<CGAffineTransform>(),
        48,
        "CGAffineTransform size drifted"
    );
    assert_eq!(
        align_of::<CGAffineTransform>(),
        8,
        "CGAffineTransform alignment drifted"
    );
}

#[cfg(feature = "cv")]
#[test]
fn cv_value_type_layout() {
    use apple_cf::cv::{CVImageRect, CVImageSize, CVPixelBufferLockFlags};

    assert_eq!(size_of::<CVImageSize>(), 16, "CVImageSize size drifted");
    assert_eq!(
        align_of::<CVImageSize>(),
        8,
        "CVImageSize alignment drifted"
    );

    assert_eq!(size_of::<CVImageRect>(), 32, "CVImageRect size drifted");
    assert_eq!(
        align_of::<CVImageRect>(),
        8,
        "CVImageRect alignment drifted"
    );

    assert_eq!(
        size_of::<CVPixelBufferLockFlags>(),
        8,
        "CVPixelBufferLockFlags size drifted"
    );
    assert_eq!(
        align_of::<CVPixelBufferLockFlags>(),
        8,
        "CVPixelBufferLockFlags alignment drifted"
    );
    assert_eq!(
        CVPixelBufferLockFlags::from_bits(1_u64 << 40).bits(),
        1_u64 << 40,
        "CVPixelBufferLockFlags truncated upper CVOptionFlags bits"
    );
}

#[cfg(feature = "cm")]
#[test]
fn cm_value_type_layout() {
    use apple_cf::cm::{CMSampleTimingInfo, CMTime, CMTimeRange};

    assert_eq!(size_of::<CMTime>(), 24, "CMTime size drifted");
    assert_eq!(align_of::<CMTime>(), 8, "CMTime alignment drifted");

    assert_eq!(
        size_of::<CMSampleTimingInfo>(),
        72,
        "CMSampleTimingInfo size drifted"
    );
    assert_eq!(
        align_of::<CMSampleTimingInfo>(),
        8,
        "CMSampleTimingInfo alignment drifted"
    );

    assert_eq!(size_of::<CMTimeRange>(), 48, "CMTimeRange size drifted");
    assert_eq!(
        align_of::<CMTimeRange>(),
        8,
        "CMTimeRange alignment drifted"
    );
}

/// Runtime cross-check that aggregates every enabled-framework value type.
/// A `false` return means the Rust layout no longer matches the values pinned
/// by the compile-time assertions in `apple_cf::ffi`, which is a real ABI bug.
#[test]
fn ffi_layout_matches_pinned_values() {
    assert!(
        apple_cf::ffi::verify_ffi_layout(),
        "an FFI value type's size/alignment drifted from its pinned ABI layout"
    );
}
