#![cfg(all(feature = "cm", feature = "cv"))]

use apple_cf::cm::{CMFormatDescription, CMSampleBuffer};
use apple_cf::cv::CVPixelBuffer;
use apple_cf::raw;

#[test]
fn sample_buffer_image_getters_preserve_ownership() {
    let pixel_buffer = CVPixelBuffer::create(8, 4, 0x4247_5241).expect("pixel buffer");

    let mut format_ptr: raw::CMVideoFormatDescriptionRef = std::ptr::null();
    let format_status = unsafe {
        raw::CMVideoFormatDescriptionCreateForImageBuffer(
            std::ptr::null(),
            pixel_buffer.as_ptr().cast(),
            &raw mut format_ptr,
        )
    };
    assert_eq!(format_status, 0);
    let format = unsafe { CMFormatDescription::from_raw(format_ptr.cast_mut().cast()) }
        .expect("format description");

    let timing = raw::CMSampleTimingInfo {
        duration: raw::CMTime {
            value: 1,
            timescale: 30,
            flags: 1,
            epoch: 0,
        },
        presentationTimeStamp: raw::CMTime {
            value: 0,
            timescale: 1,
            flags: 1,
            epoch: 0,
        },
        decodeTimeStamp: raw::CMTime {
            value: 0,
            timescale: 0,
            flags: 0,
            epoch: 0,
        },
    };
    let mut sample_ptr = std::ptr::null_mut();
    let sample_status = unsafe {
        raw::CMSampleBufferCreateReadyWithImageBuffer(
            std::ptr::null(),
            pixel_buffer.as_ptr().cast(),
            format.as_ptr().cast(),
            &raw const timing,
            &raw mut sample_ptr,
        )
    };
    assert_eq!(sample_status, 0);
    let sample = unsafe { CMSampleBuffer::from_raw(sample_ptr.cast()) }.expect("sample buffer");

    assert_eq!(sample.image_buffer_ptr_borrowed(), pixel_buffer.as_ptr());
    let image = sample.image_buffer().expect("owned image buffer");
    assert_eq!(image.as_ptr(), pixel_buffer.as_ptr());

    drop((sample, pixel_buffer));
    assert!((image.encoded_size().width - 8.0).abs() < f64::EPSILON);
    assert!((image.encoded_size().height - 4.0).abs() < f64::EPSILON);
}
