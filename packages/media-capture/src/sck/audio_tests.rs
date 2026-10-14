use super::*;
use std::ffi::c_void;
use std::ptr::{null, null_mut};

unsafe extern "C" {
    fn CMAudioFormatDescriptionCreate(
        allocator: *const c_void,
        asbd: *const ffi::AudioStreamBasicDescription,
        layout_size: usize,
        layout: *const c_void,
        cookie_size: usize,
        cookie: *const c_void,
        extensions: *const c_void,
        out: *mut ffi::CMFormatDescriptionRef,
    ) -> i32;
    fn CMSampleBufferCreate(
        allocator: *const c_void,
        block: *mut c_void,
        ready: u8,
        callback: *const c_void,
        context: *mut c_void,
        format: ffi::CMFormatDescriptionRef,
        samples: isize,
        timing_count: isize,
        timing: *const c_void,
        size_count: isize,
        sizes: *const usize,
        out: *mut ffi::CMSampleBufferRef,
    ) -> i32;
    fn CMSampleBufferSetDataBufferFromAudioBufferList(
        sb: ffi::CMSampleBufferRef,
        allocator: *const c_void,
        memory_allocator: *const c_void,
        flags: u32,
        list: *const ffi::StereoAudioBufferList,
    ) -> i32;
    fn CMSampleBufferSetDataReady(sb: ffi::CMSampleBufferRef) -> i32;
}

struct Owned(*mut c_void);
#[repr(C)]
struct Timing {
    duration: ffi::CMTime,
    pts: ffi::CMTime,
    dts: ffi::CMTime,
}
impl Drop for Owned {
    fn drop(&mut self) {
        unsafe { core_foundation::base::CFRelease(self.0.cast()) };
    }
}

fn sample(planar: bool, rate: f64, ready: bool) -> Owned {
    let frame_bytes = if planar { 4 } else { 8 };
    let asbd = ffi::AudioStreamBasicDescription {
        sample_rate: rate,
        format_id: ffi::K_AUDIO_FORMAT_LINEAR_PCM,
        // Packed f32, with one channel per buffer when planar.
        format_flags: ffi::K_AUDIO_FORMAT_FLAG_IS_FLOAT
            | (1 << 3)
            | if planar {
                ffi::K_AUDIO_FORMAT_FLAG_IS_NON_INTERLEAVED
            } else {
                0
            },
        bytes_per_packet: frame_bytes,
        frames_per_packet: 1,
        bytes_per_frame: frame_bytes,
        channels_per_frame: 2,
        bits_per_channel: 32,
        reserved: 0,
    };
    let mut desc = null_mut();
    assert_eq!(
        unsafe {
            CMAudioFormatDescriptionCreate(null(), &asbd, 0, null(), 0, null(), null(), &mut desc)
        },
        0
    );
    let desc = Owned(desc);
    let mut sb = null_mut();
    let timing = Timing {
        duration: ffi::CMTime {
            value: 1,
            timescale: rate as i32,
            flags: 1,
            epoch: 0,
        },
        pts: ffi::CMTime {
            value: 0,
            timescale: rate as i32,
            flags: 1,
            epoch: 0,
        },
        dts: ffi::CMTime {
            value: 0,
            timescale: 0,
            flags: 0,
            epoch: 0,
        },
    };
    assert_eq!(
        unsafe {
            CMSampleBufferCreate(
                null(),
                null_mut(),
                0,
                null(),
                null_mut(),
                desc.0,
                4,
                1,
                (&timing as *const Timing).cast(),
                0,
                null(),
                &mut sb,
            )
        },
        0
    );
    let sb = Owned(sb);
    if !ready {
        return sb;
    }
    let mut values: Vec<f32> = if planar {
        vec![0.1, 0.2, 0.3, 0.4, -0.1, -0.2, -0.3, -0.4]
    } else {
        vec![0.1, -0.1, 0.2, -0.2, 0.3, -0.3, 0.4, -0.4]
    };
    let list = ffi::StereoAudioBufferList {
        count: if planar { 2 } else { 1 },
        buffers: [
            ffi::AudioBuffer {
                channels: if planar { 1 } else { 2 },
                byte_size: 4 * frame_bytes,
                data: values.as_mut_ptr().cast(),
            },
            ffi::AudioBuffer {
                channels: 1,
                byte_size: 16,
                data: unsafe { values.as_mut_ptr().add(4) }.cast(),
            },
        ],
    };
    assert_eq!(
        unsafe { CMSampleBufferSetDataBufferFromAudioBufferList(sb.0, null(), null(), 0, &list) },
        0
    );
    if ready {
        assert_eq!(unsafe { CMSampleBufferSetDataReady(sb.0) }, 0);
    }
    sb
}

#[test]
fn native_planar_and_interleaved_pcm_preserve_channel_order() {
    for planar in [false, true] {
        let sb = sample(planar, 48_000.0, true);
        let copied = unsafe { copy_audio_samples_ref(sb.0) }.expect("native PCM copy");
        assert_eq!(copied, [0.1, -0.1, 0.2, -0.2, 0.3, -0.3, 0.4, -0.4]);
        let mut chunker = AudioChunker::new();
        for _ in 0..SAMPLES_PER_FRAME / 4 {
            chunker.push(&copied, Some(42_000));
        }
        let frame = chunker.pop_frame().unwrap();
        assert_eq!(frame.samples.len(), SAMPLES_PER_FRAME * CHANNELS);
        assert_eq!(frame.pts_micros, 42_000);
        assert!(
            frame
                .samples
                .as_chunks::<2>()
                .0
                .iter()
                .all(|pair| pair[0] > 0 && pair[1] < 0)
        );
    }
}

#[test]
fn incompatible_sample_rate_is_rejected_instead_of_playing_at_wrong_speed() {
    let sb = sample(true, 44_100.0, true);
    assert!(unsafe { copy_audio_samples_ref(sb.0) }.is_none());
}

#[test]
fn unready_pcm_is_not_forwarded_as_silence() {
    let sb = sample(true, 48_000.0, false);
    assert!(unsafe { copy_audio_samples_ref(sb.0) }.is_none());
}
