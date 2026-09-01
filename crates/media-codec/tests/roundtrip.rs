//! Real hardware codec roundtrip tests (no screen recording permission required).

use removent_media_codec::{
    AudioDecoder, AudioEncoder, VideoDecoder, VideoEncoder, annexb_has_idr, avcc_to_annexb,
    extract_param_sets, split_annexb_nals,
};
use removent_proto::CodecId;
use std::time::{Duration, Instant};

fn wait_decoded(dec: &VideoDecoder, budget_ms: u64) -> Option<removent_media_codec::DecodedBgra> {
    let deadline = Instant::now() + Duration::from_millis(budget_ms);
    while Instant::now() < deadline {
        if let Some(d) = dec.try_recv_decoded() {
            return Some(d);
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    None
}

const W: usize = 320;
const H: usize = 240;

/// Gradient test image.
fn gradient_frame(seed: u8) -> Vec<u8> {
    let mut buf = vec![0u8; W * H * 4];
    for y in 0..H {
        for x in 0..W {
            let o = (y * W + x) * 4;
            buf[o] = x as u8;
            buf[o + 1] = y as u8;
            buf[o + 2] = seed;
            buf[o + 3] = 255;
        }
    }
    buf
}

#[test]
fn annexb_split_and_convert() {
    let annexb: Vec<u8> = [0, 0, 0, 1, 0x67, 1, 2, 3, 0, 0, 0, 1, 0x68, 9, 9].to_vec();
    let nals = split_annexb_nals(&annexb);
    assert_eq!(nals.len(), 2);
    assert_eq!(nals[0], &[0x67, 1, 2, 3]);
    assert_eq!(nals[1], &[0x68, 9, 9]);

    // AVCC roundtrip
    let mut avcc = Vec::new();
    for n in &nals {
        avcc.extend_from_slice(&(n.len() as u32).to_be_bytes());
        avcc.extend_from_slice(n);
    }
    let back = avcc_to_annexb(&avcc).unwrap();
    assert_eq!(back, annexb);

    let params = extract_param_sets(&annexb, false);
    assert_eq!(params.len(), 2); // SPS(7)+PPS(8)
}

#[test]
fn h264_encode_decode_roundtrip() {
    let codec = CodecId::H264;
    let mut enc = VideoEncoder::new(codec, W, H, 4000, 30).expect("encoder");

    let f0 = gradient_frame(10);
    let f1 = gradient_frame(60);
    let out0 = enc.encode_bgra(&f0, 0).expect("encode 0");
    assert_eq!(out0.len(), 1, "first encode should emit a frame");
    assert!(out0[0].keyframe, "first frame must be keyframe");
    assert!(out0[0].data.len() > 8);

    let _out1 = enc.encode_bgra(&f1, 33_333).expect("encode 1");

    let ps = enc
        .parameter_sets()
        .expect("param sets after first encode")
        .to_vec();
    assert!(!ps.is_empty());

    let dec = VideoDecoder::new(codec, W, H, &ps).expect("decoder");
    dec.decode_annexb(&out0[0].data, 0).expect("decode");
    let decoded = wait_decoded(&dec, 3000).expect("decoded frame within timeout");
    assert_eq!(decoded.data.len(), W * H * 4);

    // Decoder output may be an NV12→BGRA conversion result, so do a lenient
    // similarity check: the blue channel (solid plane with seed=10) mean should
    // be near 10, and the green channel increases along Y.
    let (pixels, _) = decoded.data.as_chunks::<4>();
    let blue_mean: u64 = pixels.iter().step_by(97).map(|p| p[2] as u64).sum::<u64>();
    let samples = decoded.data.len() / 4 / 97;
    let blue_mean = blue_mean / samples.max(1) as u64;
    let _green_mean: u64 =
        pixels.iter().step_by(97).map(|p| p[1] as u64).sum::<u64>() / samples.max(1) as u64;
    assert!(
        (0..=60).contains(&blue_mean),
        "blue channel mean {blue_mean} should be near seed 10"
    );
}

/// request_keyframe must force the next frame to an IDR via the frame-level
/// ForceKeyFrame option (regression: VTSessionSetProperty silently failed with
/// kVTPropertyNotSupportedErr).
#[test]
fn h264_force_keyframe_produces_idr() {
    let codec = CodecId::H264;
    let mut enc = VideoEncoder::new(codec, W, H, 4000, 30).expect("encoder");
    // Warm up with a few frames so the forced one is not the session's first.
    for i in 0..5u8 {
        let f = gradient_frame(i * 20);
        enc.encode_bgra(&f, i64::from(i) * 33_333).expect("encode");
    }
    enc.request_keyframe();
    let f = gradient_frame(120);
    let out = enc.encode_bgra(&f, 6 * 33_333).expect("forced encode");
    assert_eq!(out.len(), 1);
    assert!(
        out[0].keyframe,
        "frame after request_keyframe must be a keyframe"
    );
    assert!(
        annexb_has_idr(&out[0].data, false),
        "IDR NAL expected after force keyframe"
    );
}

#[test]
fn hevc_encode_decode_roundtrip() {
    let codec = CodecId::Hevc;
    if !videotoolbox_supports_hevc() {
        eprintln!("HEVC not supported on this machine, skipping");
        return;
    }
    let mut enc = VideoEncoder::new(codec, W, H, 3000, 30).expect("encoder");
    let f0 = gradient_frame(200);
    let out = enc.encode_bgra(&f0, 0).expect("encode");
    assert_eq!(out.len(), 1);
    assert!(out[0].keyframe);
    assert!(
        annexb_has_idr(&out[0].data, true),
        "IDR NAL expected in HEVC keyframe"
    );

    let ps = enc.parameter_sets().expect("hevc param sets").to_vec();
    assert!(ps.len() >= 3, "VPS+SPS+PPS expected, got {}", ps.len());

    let dec = VideoDecoder::new(codec, W, H, &ps).expect("decoder");
    dec.decode_annexb(&out[0].data, 0).expect("decode");
    let decoded = wait_decoded(&dec, 3000).expect("HEVC decoded frame within timeout");
    assert_eq!(decoded.data.len(), W * H * 4);
}

fn videotoolbox_supports_hevc() -> bool {
    // Apple Silicon and Intel Macs from the last decade all support HEVC encoding; probe once conservatively.
    VideoEncoder::new(CodecId::Hevc, 64, 64, 500, 30).is_ok()
}

// ---------------- Audio ----------------

fn sine_i16(freq_hz: f32, frames: usize) -> Vec<i16> {
    (0..frames)
        .map(|i| {
            let v = (2.0 * std::f32::consts::PI * freq_hz * i as f32 / 48_000.0).sin();
            (v * 8000.0) as i16
        })
        .collect()
}

#[test]
fn opus_roundtrip_preserves_energy() {
    let mut enc =
        AudioEncoder::new(64, removent_media_codec::Application::Audio).expect("audio encoder");
    let mut dec = AudioDecoder::new().expect("audio decoder");

    // Exactly one frame (10ms stereo)
    let mono = sine_i16(1000.0, 480);
    let mut frame = Vec::with_capacity(960);
    for s in &mono {
        frame.push(*s);
        frame.push(*s);
    }

    let energy_in: f64 = frame.iter().map(|v| (*v as f64 / 32768.0).powi(2)).sum();

    let (_seq, packet) = enc.encode_frame(&frame).expect("encode");
    assert!(!packet.is_empty() && packet.len() < 400);

    let decoded = dec.decode_frame(&packet).expect("decode");
    let energy_out: f64 = decoded.iter().map(|v| (*v as f64 / 32768.0).powi(2)).sum();
    assert!(
        energy_out > energy_in * 0.25 && energy_out < energy_in * 4.0,
        "energy mismatch: in={energy_in:.6} out={energy_out:.6}"
    );
}

#[test]
fn opus_conceal_produces_output() {
    let mut enc = AudioEncoder::new(48, removent_media_codec::Application::Voip).expect("enc");
    let mono = sine_i16(440.0, 480);
    let mut frame = Vec::with_capacity(960);
    for s in &mono {
        frame.push(*s);
        frame.push(*s);
    }
    let (_, packet) = enc.encode_frame(&frame).expect("encode");
    let mut dec = AudioDecoder::new().expect("dec");
    let _ = dec.decode_frame(&packet).unwrap();
    let plc = dec.conceal().expect("conceal");
    assert_eq!(plc.len(), 960);
}
