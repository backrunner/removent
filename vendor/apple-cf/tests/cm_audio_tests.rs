#![cfg(feature = "cm")]

use apple_cf::cm::{CMBlockBuffer, CMFormatDescription, CMSampleBuffer};
use apple_cf::raw;
use std::io::Read;

const SPS: [u8; 14] = [
    0x67, 0x42, 0x00, 0x1e, 0x95, 0xa8, 0x28, 0x0f, 0x00, 0x44, 0xfc, 0xb8, 0x08, 0x80,
];
const PPS: [u8; 4] = [0x68, 0xce, 0x06, 0xe2];

fn pcm_bytes(samples: &[i16]) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|sample| sample.to_ne_bytes())
        .collect()
}

fn pcm_format(channels: u32) -> CMFormatDescription {
    let asbd = raw::AudioStreamBasicDescription {
        mSampleRate: 48_000.0,
        mFormatID: u32::from_be_bytes(*b"lpcm"),
        mFormatFlags: 0x4 | 0x8,
        mBytesPerPacket: 2 * channels,
        mFramesPerPacket: 1,
        mBytesPerFrame: 2 * channels,
        mChannelsPerFrame: channels,
        mBitsPerChannel: 16,
        mReserved: 0,
    };
    let mut format: raw::CMAudioFormatDescriptionRef = std::ptr::null();
    let status = unsafe {
        raw::CMAudioFormatDescriptionCreate(
            std::ptr::null(),
            &raw const asbd,
            0,
            std::ptr::null(),
            0,
            std::ptr::null(),
            std::ptr::null(),
            &raw mut format,
        )
    };
    assert_eq!(status, 0);
    unsafe { CMFormatDescription::from_raw(format.cast_mut().cast()) }.expect("audio format")
}

fn pcm_sample_buffer(samples: &[i16], channels: u32) -> CMSampleBuffer {
    let format = pcm_format(channels);
    let block = CMBlockBuffer::create(&pcm_bytes(samples)).expect("block buffer");
    let frames = samples.len() / usize::try_from(channels).expect("channel count");
    let mut sample: raw::CMSampleBufferRef = std::ptr::null_mut();
    let status = unsafe {
        raw::CMAudioSampleBufferCreateWithPacketDescriptions(
            std::ptr::null(),
            block.as_ptr().cast(),
            1,
            None,
            std::ptr::null_mut(),
            format.as_ptr().cast(),
            i64::try_from(frames).expect("frame count"),
            raw::CMTime {
                value: 0,
                timescale: 48_000,
                flags: 1,
                epoch: 0,
            },
            std::ptr::null(),
            &raw mut sample,
        )
    };
    assert_eq!(status, 0);
    unsafe { CMSampleBuffer::from_raw(sample.cast()) }.expect("sample buffer")
}

#[test]
fn audio_sample_buffer_exposes_a_self_owned_audio_buffer_list() {
    let samples = [1_i16, -1, 2, -2, 3, -3, 4, -4];
    let sample = pcm_sample_buffer(&samples, 2);
    let list = sample.audio_buffer_list().expect("audio buffer list");
    drop(sample);
    assert_eq!(list.num_buffers(), 1);
    assert_eq!(list.iter().count(), 1);
    assert!(list.get(1).is_none());
    let buffer = list.get(0).expect("first buffer");
    assert_eq!(buffer.number_channels, 2);
    assert_eq!(buffer.data_byte_size(), 16);
    assert_eq!(buffer.data(), pcm_bytes(&samples).as_slice());
    assert_eq!(
        list.buffer(0).expect("buffer ref").data(),
        pcm_bytes(&samples).as_slice()
    );
}

#[test]
fn audio_buffer_list_is_an_error_for_non_audio_samples() {
    let block = CMBlockBuffer::create(&[0, 1, 2]).expect("block buffer");
    let sample_size = 3_usize;
    let mut sample: raw::CMSampleBufferRef = std::ptr::null_mut();
    let status = unsafe {
        raw::CMSampleBufferCreateReady(
            std::ptr::null(),
            block.as_ptr().cast(),
            std::ptr::null(),
            1,
            0,
            std::ptr::null(),
            1,
            &raw const sample_size,
            &raw mut sample,
        )
    };
    assert_eq!(status, 0);
    let sample = unsafe { CMSampleBuffer::from_raw(sample.cast()) }.expect("sample buffer");
    assert!(sample.audio_buffer_list().is_err());
}

#[test]
fn sample_attachments_report_sync_samples() {
    let sample = pcm_sample_buffer(&[0, 0], 1);
    assert!(sample.is_sync_sample());

    let attachments = unsafe { raw::CMSampleBufferGetSampleAttachmentsArray(sample.as_ptr().cast(), 1) };
    assert!(!attachments.is_null());
    let first = unsafe { raw::CFArrayGetValueAtIndex(attachments, 0) };
    unsafe {
        raw::CFDictionarySetValue(
            first.cast_mut().cast(),
            raw::kCMSampleAttachmentKey_NotSync.cast(),
            raw::kCFBooleanTrue.cast(),
        );
    }
    assert!(!sample.is_sync_sample());
    assert!(!sample.sample_attachments().is_empty());
    assert_eq!(sample.sample_attachments()[0].len(), 1);

    unsafe {
        raw::CFDictionarySetValue(
            first.cast_mut().cast(),
            raw::kCMSampleAttachmentKey_NotSync.cast(),
            raw::kCFBooleanFalse.cast(),
        );
    }
    assert!(sample.is_sync_sample());
}

#[test]
fn h264_format_description_exposes_dimensions_and_parameter_sets() {
    let parameter_sets = [SPS.as_ptr(), PPS.as_ptr()];
    let sizes = [SPS.len(), PPS.len()];
    let mut format: raw::CMFormatDescriptionRef = std::ptr::null();
    let status = unsafe {
        raw::CMVideoFormatDescriptionCreateFromH264ParameterSets(
            std::ptr::null(),
            2,
            parameter_sets.as_ptr(),
            sizes.as_ptr(),
            4,
            &raw mut format,
        )
    };
    assert_eq!(status, 0);
    let format =
        unsafe { CMFormatDescription::from_raw(format.cast_mut().cast()) }.expect("format");
    let (width, height) = format.video_dimensions().expect("video dimensions");
    assert!(width > 0 && height > 0);
    let sets = format.video_parameter_sets().expect("parameter sets");
    assert_eq!(sets.parameter_sets, vec![SPS.to_vec(), PPS.to_vec()]);
    assert_eq!(sets.nal_unit_header_length, 4);
}

#[test]
fn audio_format_description_has_no_video_properties() {
    let format = pcm_format(2);
    assert_eq!(format.video_dimensions(), None);
    assert!(format.video_parameter_sets().is_err());
}

#[test]
fn block_buffer_cursor_copies_and_slice_views_are_explicit() {
    let data = [9_u8, 8, 7, 6];
    let block = CMBlockBuffer::create(&data).expect("block buffer");
    let mut copied = Vec::new();
    block
        .cursor()
        .expect("cursor")
        .read_to_end(&mut copied)
        .expect("read");
    assert_eq!(copied, data);
    assert_eq!(unsafe { block.as_slice() }, Some(&data[..]));
    let mut borrowed = Vec::new();
    unsafe { block.cursor_ref() }
        .expect("cursor ref")
        .read_to_end(&mut borrowed)
        .expect("read");
    assert_eq!(borrowed, data);
}
