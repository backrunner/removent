//! Repeatable local codec benchmark.
//!
//! Run with:
//!   cargo run --release -p removent-media-codec --example benchmark
//!
//! Video numbers measure synchronous BGRA submission cost and per-frame time
//! from encoder submission to decoded output. Audio numbers measure Opus
//! encode/decode cost for 10 ms stereo frames. This is intentionally a small
//! benchmark harness rather than a statistically heavy microbenchmark.

use removent_media_codec::{
    Application, AudioDecoder, AudioEncoder, VideoDecoder, VideoEncoder, VideoError,
    av1_hardware_support,
};
use removent_proto::CodecId;
use std::collections::HashMap;
use std::time::{Duration, Instant};

const WIDTH: usize = 1280;
const HEIGHT: usize = 720;
const VIDEO_FRAMES: usize = 120;
const AUDIO_FRAMES: usize = 1_000;

fn video_frame(seed: u8) -> Vec<u8> {
    let mut frame = vec![0u8; WIDTH * HEIGHT * 4];
    for (i, px) in frame.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let x = i % WIDTH;
        let y = i / WIDTH;
        px[0] = (x as u8).wrapping_add(seed);
        px[1] = (y as u8).wrapping_add(seed.wrapping_mul(3));
        px[2] = seed;
        px[3] = 255;
    }
    frame
}

fn percentile(sorted: &[u128], p: usize, q: usize) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    let index = ((sorted.len().saturating_sub(1)) * p / q).min(sorted.len().saturating_sub(1));
    sorted[index]
}

fn machine_model() -> String {
    std::process::Command::new("sysctl")
        .args(["-n", "hw.model"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

fn drain_decoded(decoder: &VideoDecoder) -> Vec<removent_media_codec::DecodedBgra> {
    let mut frames = Vec::new();
    while let Some(frame) = decoder.try_recv_decoded() {
        frames.push(frame);
    }
    frames
}

fn wait_decoded(
    decoder: &VideoDecoder,
    timeout: Duration,
) -> Vec<removent_media_codec::DecodedBgra> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let frames = drain_decoded(decoder);
        if !frames.is_empty() {
            return frames;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    Vec::new()
}

fn account_decoded(
    frames: Vec<removent_media_codec::DecodedBgra>,
    submitted_at: &mut HashMap<i64, Instant>,
    moving_decoded: &mut usize,
    roundtrip_timings: &mut Vec<u128>,
) {
    for frame in frames {
        if let Some(started) = submitted_at.remove(&frame.pts_us) {
            roundtrip_timings.push(started.elapsed().as_nanos());
            *moving_decoded += 1;
        }
    }
}

fn run_video(codec: CodecId, frame: &[u8]) -> Result<(), VideoError> {
    let mut current_frame = frame.to_vec();
    let mut encoder = VideoEncoder::new(codec, WIDTH, HEIGHT, 8_000, 60)?;
    let warmup = encoder.encode_bgra(frame, 0)?;
    let parameter_sets = match codec {
        CodecId::Av1 => Vec::new(),
        _ => encoder
            .parameter_sets()
            .ok_or(VideoError::NoParameterSets)?
            .to_vec(),
    };
    let decoder = VideoDecoder::new(codec, WIDTH, HEIGHT, &parameter_sets)?;
    let mut roundtrip_timings = Vec::with_capacity(VIDEO_FRAMES);
    let mut submitted_at = HashMap::with_capacity(VIDEO_FRAMES);
    let mut moving_decoded = 0usize;
    for encoded in warmup {
        decoder.decode_annexb(&encoded.data, encoded.pts_us)?;
        let _ = wait_decoded(&decoder, Duration::from_secs(3));
    }
    let mut timings = Vec::with_capacity(VIDEO_FRAMES);
    let mut encoded_bytes = 0usize;
    let started = Instant::now();
    for n in 0..VIDEO_FRAMES {
        // Keep the workload mostly static but introduce a moving marker so
        // inter-frame compression is exercised without timing frame creation.
        let marker = (n * 4099) % (WIDTH * HEIGHT);
        current_frame[marker * 4] = n as u8;
        current_frame[marker * 4 + 1] = n.wrapping_mul(3) as u8;
        let pts_us = (n as i64 + 1) * 16_667;
        let t0 = Instant::now();
        submitted_at.insert(pts_us, t0);
        let outputs = encoder.encode_bgra(&current_frame, pts_us)?;
        timings.push(t0.elapsed().as_nanos());
        encoded_bytes += outputs
            .iter()
            .filter(|f| submitted_at.contains_key(&f.pts_us))
            .map(|f| f.data.len())
            .sum::<usize>();
        for encoded in outputs {
            decoder.decode_annexb(&encoded.data, encoded.pts_us)?;
            let decoded = wait_decoded(&decoder, Duration::from_secs(3));
            account_decoded(
                decoded,
                &mut submitted_at,
                &mut moving_decoded,
                &mut roundtrip_timings,
            );
        }
    }
    let tail = encoder.flush()?;
    encoded_bytes += tail
        .iter()
        .filter(|f| submitted_at.contains_key(&f.pts_us))
        .map(|f| f.data.len())
        .sum::<usize>();
    for encoded in tail {
        decoder.decode_annexb(&encoded.data, encoded.pts_us)?;
        let decoded = wait_decoded(&decoder, Duration::from_secs(3));
        account_decoded(
            decoded,
            &mut submitted_at,
            &mut moving_decoded,
            &mut roundtrip_timings,
        );
    }
    decoder.flush()?;
    // rav1d may finish the last few pictures just after the final submission;
    // drain its asynchronous output before reporting the moving-marker count.
    let drain_deadline = Instant::now() + Duration::from_secs(3);
    while moving_decoded < VIDEO_FRAMES && Instant::now() < drain_deadline {
        let decoded = drain_decoded(&decoder);
        if decoded.is_empty() {
            std::thread::sleep(Duration::from_millis(1));
        } else {
            account_decoded(
                decoded,
                &mut submitted_at,
                &mut moving_decoded,
                &mut roundtrip_timings,
            );
        }
    }
    let moving_wall = started.elapsed();
    // A static workload models a desktop that receives repeated captures while
    // the user is reading. Exact equality is the same policy used by the host
    // sender; skipped frames never enter VideoToolbox or the wire.
    let static_started = Instant::now();
    let mut static_last: Option<Vec<u8>> = None;
    let mut static_encoded = 0usize;
    let mut static_skipped = 0usize;
    let mut static_bytes = 0usize;
    let mut static_timings = Vec::new();
    let mut static_encoder = VideoEncoder::new(codec, WIDTH, HEIGHT, 8_000, 60)?;
    for n in 0..VIDEO_FRAMES {
        if static_last.as_deref() == Some(current_frame.as_slice()) {
            static_skipped += 1;
            continue;
        }
        let t0 = Instant::now();
        let outputs = static_encoder.encode_bgra(
            &current_frame,
            (VIDEO_FRAMES as i64 + n as i64 + 1) * 16_667,
        )?;
        static_timings.push(t0.elapsed().as_nanos());
        if outputs.is_empty() {
            continue;
        }
        static_encoded += outputs.len();
        static_bytes += outputs.iter().map(|f| f.data.len()).sum::<usize>();
        static_last = Some(current_frame.clone());
        // Static mode measures capture deduplication and wire volume. The
        // moving-marker decoder was flushed above, so no inter-frame decode is
        // attempted against its reset state here.
    }
    let static_tail = static_encoder.flush()?;
    static_encoded += static_tail.len();
    static_bytes += static_tail.iter().map(|f| f.data.len()).sum::<usize>();
    let static_wall = static_started.elapsed();
    static_timings.sort_unstable();
    timings.sort_unstable();
    roundtrip_timings.sort_unstable();
    let raw_bytes = frame.len() * VIDEO_FRAMES;
    let seconds = moving_wall.as_secs_f64().max(f64::MIN_POSITIVE);
    let fps = VIDEO_FRAMES as f64 / seconds;
    let mbps = encoded_bytes as f64 * 8.0 / seconds / 1_000_000.0;
    let ratio = raw_bytes as f64 / encoded_bytes.max(1) as f64;
    println!(
        "video codec={codec:?} workload=moving-marker {WIDTH}x{HEIGHT}: frames={VIDEO_FRAMES} fps={fps:.1} encode_p50={:.3}ms encode_p95={:.3}ms encode_decode_p50={:.3}ms encode_decode_p95={:.3}ms decoded={}/{} encoded={:.2}MB throughput={mbps:.2}Mbps compression={ratio:.1}x",
        percentile(&timings, 50, 100) as f64 / 1_000_000.0,
        percentile(&timings, 95, 100) as f64 / 1_000_000.0,
        percentile(&roundtrip_timings, 50, 100) as f64 / 1_000_000.0,
        percentile(&roundtrip_timings, 95, 100) as f64 / 1_000_000.0,
        moving_decoded,
        VIDEO_FRAMES,
        encoded_bytes as f64 / 1_000_000.0,
    );
    println!(
        "video codec={codec:?} workload=static {WIDTH}x{HEIGHT}: captured={VIDEO_FRAMES} encoded={static_encoded} skipped_unchanged={static_skipped} skip_ratio={:.1}% encode_p50={:.3}ms wire={:.2}MB wall={:.3}s",
        static_skipped as f64 * 100.0 / VIDEO_FRAMES as f64,
        percentile(&static_timings, 50, 100) as f64 / 1_000_000.0,
        static_bytes as f64 / 1_000_000.0,
        static_wall.as_secs_f64(),
    );
    Ok(())
}

fn run_audio() -> Result<(), Box<dyn std::error::Error>> {
    let mut encoder = AudioEncoder::new(64, Application::Audio)?;
    let mut decoder = AudioDecoder::new()?;
    let mut pcm = Vec::with_capacity(removent_media_codec::FRAME_SAMPLES_PER_CHANNEL * 2);
    for i in 0..removent_media_codec::FRAME_SAMPLES_PER_CHANNEL {
        let sample =
            (8000.0 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin()) as i16;
        pcm.extend_from_slice(&[sample, sample]);
    }
    let mut encode_timings = Vec::with_capacity(AUDIO_FRAMES);
    let mut decode_timings = Vec::with_capacity(AUDIO_FRAMES);
    let mut encoded_bytes = 0usize;
    let started = Instant::now();
    for _ in 0..AUDIO_FRAMES {
        let t0 = Instant::now();
        let (_, packet) = encoder.encode_frame(&pcm)?;
        encode_timings.push(t0.elapsed().as_nanos());
        encoded_bytes += packet.len();
        let t1 = Instant::now();
        let _ = decoder.decode_frame(&packet)?;
        decode_timings.push(t1.elapsed().as_nanos());
    }
    let wall = started.elapsed();
    encode_timings.sort_unstable();
    decode_timings.sort_unstable();
    let media_seconds = AUDIO_FRAMES as f64 * 0.010;
    let realtime = media_seconds / wall.as_secs_f64().max(f64::MIN_POSITIVE);
    let kbps = encoded_bytes as f64 * 8.0 / media_seconds / 1_000.0;
    println!(
        "audio opus workload=440Hz-sine 48kHz stereo/10ms: frames={AUDIO_FRAMES} encode_p50={:.3}ms encode_p95={:.3}ms decode_p50={:.3}ms decode_p95={:.3}ms bitrate={kbps:.1}kbps realtime={realtime:.1}x",
        percentile(&encode_timings, 50, 100) as f64 / 1_000_000.0,
        percentile(&encode_timings, 95, 100) as f64 / 1_000_000.0,
        percentile(&decode_timings, 50, 100) as f64 / 1_000_000.0,
        percentile(&decode_timings, 95, 100) as f64 / 1_000_000.0,
    );
    Ok(())
}

fn main() {
    println!(
        "environment model={} os={} arch={} build={} threads={}",
        machine_model(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        std::thread::available_parallelism().map_or(1, usize::from),
    );
    let av1 = av1_hardware_support();
    println!(
        "av1 capability decoder_hardware={} encoder_hardware={} software_encoder=true software_decoder=true protocol_enabled=true opt_in=REMOVENT_VIDEO_CODEC=av1",
        av1.decoder, av1.encoder
    );
    let frame = video_frame(37);
    for codec in [CodecId::H264, CodecId::Hevc, CodecId::Av1] {
        if let Err(err) = run_video(codec, &frame) {
            println!("video codec={codec:?}: unavailable ({err})");
        }
    }
    if let Err(err) = run_audio() {
        eprintln!("audio benchmark failed: {err}");
        std::process::exit(1);
    }
}
