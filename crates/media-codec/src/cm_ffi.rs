//! Supplementary CoreMedia / CoreVideo FFI: parameter-set extraction, format
//! description creation, sample buffer creation, attachment reading.
//!
//! The few CoreMedia entry points not covered by the videotoolbox crate are
//! declared here directly; linking relies on the CoreMedia framework already
//! pulled in by videotoolbox.

#![allow(non_snake_case)]

use apple_cf::cm::CMTime;
use std::os::raw::{c_int, c_void};

pub type CFStringRef = *const c_void;
pub type CFTypeRef = *const c_void;
pub type CFAllocatorRef = *const c_void;
pub type CMFormatDescriptionRef = *mut c_void;
pub type CMSampleBufferRef = *mut c_void;
pub type CMBlockBufferRef = *mut c_void;
pub type OSStatus = c_int;

/// Single-sample timing info (C layout matches CMSampleTimingInfo).
#[repr(C)]
pub struct CMSampleTimingInfo {
    pub duration: CMTime,
    pub presentation_time_stamp: CMTime,
    pub decode_time_stamp: CMTime,
}

impl CMSampleTimingInfo {
    /// Single-sample timing on a microsecond timescale.
    #[must_use]
    pub fn from_us(pts_us: i64) -> Self {
        let t = CMTime {
            value: pts_us,
            timescale: 1_000_000,
            flags: 1,
            epoch: 0,
        };
        Self {
            duration: CMTime::INVALID,
            presentation_time_stamp: t,
            decode_time_stamp: CMTime::INVALID,
        }
    }
}

unsafe extern "C" {
    /// kCMSampleAttachmentKey_NotSync: a value equal to kCFBooleanFalse means the
    /// frame is a sync frame (keyframe).
    pub static kCMSampleAttachmentKey_NotSync: CFStringRef;

    /// kVTEncodeFrameOptionKey_ForceKeyFrame ("ForceKeyFrame"): frame-level
    /// option for VTCompressionSessionEncodeFrame.
    pub static kVTEncodeFrameOptionKey_ForceKeyFrame: CFStringRef;

    pub fn CMSampleBufferGetSampleAttachmentsArray(
        sbuf: CMSampleBufferRef,
        createIfNecessary: u8,
    ) -> *const c_void;

    pub fn CFArrayGetCount(array: *const c_void) -> isize;

    pub fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;

    pub fn CFDictionaryGetValue(d: *const c_void, key: *const c_void) -> *const c_void;

    pub fn CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
        videoDesc: CMFormatDescriptionRef,
        parameterSetIndex: usize,
        parameterSetOut: *mut *const u8,
        parameterSetSizeOut: *mut usize,
        parameterSetCountOut: *mut usize,
        NALUnitHeaderLengthOut: *mut i32,
    ) -> OSStatus;

    pub fn CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(
        videoDesc: CMFormatDescriptionRef,
        parameterSetIndex: usize,
        parameterSetOut: *mut *const u8,
        parameterSetSizeOut: *mut usize,
        parameterSetCountOut: *mut usize,
        NALUnitHeaderLengthOut: *mut i32,
    ) -> OSStatus;

    pub fn CMVideoFormatDescriptionCreateFromH264ParameterSets(
        allocator: CFAllocatorRef,
        parameterSetCount: usize,
        parameterSetPointers: *const *const u8,
        parameterSetSizes: *const usize,
        NALUnitHeaderLength: i32,
        formatDescriptionOut: *mut CMFormatDescriptionRef,
    ) -> OSStatus;

    /// Note: unlike the H264 variant, this function has an extra extensions parameter.
    pub fn CMVideoFormatDescriptionCreateFromHEVCParameterSets(
        allocator: CFAllocatorRef,
        parameterSetCount: usize,
        parameterSetPointers: *const *const u8,
        parameterSetSizes: *const usize,
        NALUnitHeaderLength: i32,
        extensions: *const c_void,
        formatDescriptionOut: *mut CMFormatDescriptionRef,
    ) -> OSStatus;

    pub fn CMBlockBufferCreateWithMemoryBlock(
        structureAllocator: CFAllocatorRef,
        memoryBlock: *mut c_void,
        blockLength: usize,
        blockAllocator: CFAllocatorRef,
        customBlockSource: *const c_void,
        offsetToData: usize,
        dataLength: usize,
        flags: u32,
        newBlockBufferOut: *mut CMBlockBufferRef,
    ) -> OSStatus;

    pub fn CMBlockBufferReplaceDataBytes(
        sourceBytes: *const c_void,
        destinationBuffer: CMBlockBufferRef,
        offsetIntoDestination: usize,
        dataLength: usize,
    ) -> OSStatus;

    pub fn CMSampleBufferCreateReady(
        allocator: CFAllocatorRef,
        dataBuffer: CMBlockBufferRef,
        formatDescription: CMFormatDescriptionRef,
        sampleCount: isize,
        sampleTimingEntryCount: usize,
        sampleTimingArray: *const CMSampleTimingInfo,
        sampleSizeEntryCount: usize,
        sampleSizeArray: *const usize,
        sampleBufferOut: *mut CMSampleBufferRef,
    ) -> OSStatus;
}

pub fn default_allocator() -> CFAllocatorRef {
    unsafe { videotoolbox::ffi::kCFAllocatorDefault.cast() }
}
