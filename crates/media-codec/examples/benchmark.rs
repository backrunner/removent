//! Repeatable local codec benchmark.
//!
//! Run with:
//!   cargo run --release -p removent-media-codec --example benchmark
//!
//! Video numbers measure the synchronous cost of submitting BGRA frames to
//! VideoToolbox and draining returned samples. Audio numbers measure Opus
//! encode/decode cost for 10 ms stereo frames. This is intentionally a small,
//! dependency-free benchmark rather than a statistically heavy microbenchmark.

use removent_media_codec::{
    Application, AudioDecoder, AudioEncoder, VideoDecoder, VideoEncoder, VideoError,
};
use removent_proto::CodecId;
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

fn wait_decoded(decoder: &VideoDecoder, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if decoder.try_recv_decoded().is_some() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    false
}

fn run_video(codec: CodecId, frame: &[u8]) -> Result<(), VideoError> {
    let mut current_frame = frame.to_vec();
    let mut encoder = VideoEncoder::new(codec, WIDTH, HEIGHT, 8_000, 60)?;
    let warmup = encoder.encode_bgra(frame, 0)?;
    let parameter_sets = encoder
        .parameter_sets()
        .ok_or(VideoError::NoParameterSets)?
        .to_vec();
    let decoder = VideoDecoder::new(codec, WIDTH, HEIGHT, &parameter_sets)?;
    if let Some(encoded) = warmup.first() {
        decoder.decode_annexb(&encoded.data, encoded.pts_us)?;
        let _ = wait_decoded(&decoder, Duration::from_secs(3));
    }
    let mut timings = Vec::with_capacity(VIDEO_FRAMES);
    let mut roundtrip_timings = Vec::with_capacity(VIDEO_FRAMES);
    let mut encoded_bytes = 0usize;
    let started = Instant::now();
    for n in 0..VIDEO_FRAMES {
        // Keep the workload mostly static but introduce a moving marker so
        // inter-frame compression is exercised without timing frame creation.
        let marker = (n * 4099) % (WIDTH * HEIGHT);
        current_frame[marker * 4] = n as u8;
        current_frame[marker * 4 + 1] = n.wrapping_mul(3) as u8;
        let t0 = Instant::now();
        let outputs = encoder.encode_bgra(&current_frame, (n as i64 + 1) * 16_667)?;
        timings.push(t0.elapsed().as_nanos());
        encoded_bytes += outputs.iter().map(|f| f.data.len()).sum::<usize>();
        for encoded in outputs {
            decoder.decode_annexb(&encoded.data, encoded.pts_us)?;
            if wait_decoded(&decoder, Duration::from_secs(3)) {
                roundtrip_timings.push(t0.elapsed().as_nanos());
            }
        }
    }
    let wall = started.elapsed();
    timings.sort_unstable();
    roundtrip_timings.sort_unstable();
    let raw_bytes = frame.len() * VIDEO_FRAMES;
    let seconds = wall.as_secs_f64().max(f64::MIN_POSITIVE);
    let fps = VIDEO_FRAMES as f64 / seconds;
    let mbps = encoded_bytes as f64 * 8.0 / seconds / 1_000_000.0;
    let ratio = raw_bytes as f64 / encoded_bytes.max(1) as f64;
    println!(
        "video codec={codec:?} workload=moving-marker {WIDTH}x{HEIGHT}: frames={VIDEO_FRAMES} fps={fps:.1} encode_p50={:.3}ms encode_p95={:.3}ms encode_decode_p50={:.3}ms encode_decode_p95={:.3}ms decoded={}/{} encoded={:.2}MB throughput={mbps:.2}Mbps compression={ratio:.1}x",
        percentile(&timings, 50, 100) as f64 / 1_000_000.0,
        percentile(&timings, 95, 100) as f64 / 1_000_000.0,
        percentile(&roundtrip_timings, 50, 100) as f64 / 1_000_000.0,
        percentile(&roundtrip_timings, 95, 100) as f64 / 1_000_000.0,
        roundtrip_timings.len(),
        VIDEO_FRAMES,
        encoded_bytes as f64 / 1_000_000.0,
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
    let frame = video_frame(37);
    for codec in [CodecId::H264, CodecId::Hevc] {
        if let Err(err) = run_video(codec, &frame) {
            println!("video codec={codec:?}: unavailable ({err})");
        }
    }
    if let Err(err) = run_audio() {
        eprintln!("audio benchmark failed: {err}");
        std::process::exit(1);
    }
}
