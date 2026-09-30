//! Supplementary ScreenCaptureKit FFI: image buffer extraction and block buffer copying.

#![allow(non_snake_case)]

use std::os::raw::{c_int, c_void};

pub type CMSampleBufferRef = *mut c_void;
pub type CVPixelBufferRef = *mut c_void;
pub type CMBlockBufferRef = *mut c_void;
pub type CMFormatDescriptionRef = *mut c_void;
pub type OSStatus = c_int;

/// ABI-compatible with CoreAudio's AudioStreamBasicDescription.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct AudioStreamBasicDescription {
    pub sample_rate: f64,
    pub format_id: u32,
    pub format_flags: u32,
    pub bytes_per_packet: u32,
    pub frames_per_packet: u32,
    pub bytes_per_frame: u32,
    pub channels_per_frame: u32,
    pub bits_per_channel: u32,
    pub reserved: u32,
}

pub const K_AUDIO_FORMAT_LINEAR_PCM: u32 = u32::from_be_bytes(*b"lpcm");
pub const K_AUDIO_FORMAT_FLAG_IS_FLOAT: u32 = 1;
pub const K_AUDIO_FORMAT_FLAG_IS_NON_INTERLEAVED: u32 = 1 << 5;

/// ABI-compatible with CoreMedia's CMTime.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CMTime {
    pub value: i64,
    pub timescale: i32,
    pub flags: u32,
    pub epoch: i64,
}

unsafe extern "C" {
    pub fn CMSampleBufferGetImageBuffer(sbuf: CMSampleBufferRef) -> CVPixelBufferRef;

    pub fn CMSampleBufferGetPresentationTimeStamp(sbuf: CMSampleBufferRef) -> CMTime;

    pub fn CMSampleBufferGetDataBuffer(sbuf: CMSampleBufferRef) -> CMBlockBufferRef;

    pub fn CMSampleBufferGetFormatDescription(sbuf: CMSampleBufferRef) -> CMFormatDescriptionRef;

    pub fn CMAudioFormatDescriptionGetStreamBasicDescription(
        desc: CMFormatDescriptionRef,
    ) -> *const AudioStreamBasicDescription;

    pub fn CMBlockBufferGetDataLength(theBuffer: CMBlockBufferRef) -> usize;

    pub fn CMBlockBufferCopyDataBytes(
        theBuffer: CMBlockBufferRef,
        offsetToData: usize,
        dataLength: usize,
        destination: *mut c_void,
    ) -> OSStatus;

    pub fn CVPixelBufferLockBaseAddress(pixelBuffer: CVPixelBufferRef, lockFlags: u32) -> OSStatus;

    pub fn CVPixelBufferUnlockBaseAddress(
        pixelBuffer: CVPixelBufferRef,
        unlockFlags: u32,
    ) -> OSStatus;

    pub fn CVPixelBufferGetBaseAddress(pixelBuffer: CVPixelBufferRef) -> *mut c_void;

    pub fn CVPixelBufferGetBytesPerRow(pixelBuffer: CVPixelBufferRef) -> usize;

    pub fn CVPixelBufferGetWidth(pb: CVPixelBufferRef) -> usize;

    pub fn CVPixelBufferGetHeight(pb: CVPixelBufferRef) -> usize;
}

pub const LOCK_READ_ONLY: u32 = 0x1;
