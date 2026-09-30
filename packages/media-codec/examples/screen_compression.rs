//! Synthetic screen-content benchmark using the production VideoToolbox path.
//! cargo run --release -p removent-media-codec --example screen_compression -- /tmp/screen-compression
//! Reports bytes per presentation second, NOT bytes per benchmark wall second.
//! BGRA PSNR is a coarse round-trip check, not a text-legibility or visual score.
use removent_media_codec::{VideoDecoder, VideoEncoder};
use removent_proto::CodecId;
use std::{
    error::Error,
    io::Write,
    path::Path,
    time::{Duration, Instant},
};

const WIDTH: usize = 1920;
const HEIGHT: usize = 1080;
const FRAMES: usize = 180;
const FPS: u8 = 30;

fn desktop() -> Vec<u8> {
    let mut pixels = vec![0; WIDTH * HEIGHT * 4];
    for (i, pixel) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = (i % WIDTH, i / WIDTH);
        let glyph = ((x / 9).wrapping_mul(0x9e3779b9) ^ ((y / 20) * 431)) as u32;
        let ink = x > 60
            && x < 1800
            && y > 60
            && y % 20 < 12
            && x % 9 < 6
            && (glyph.rotate_left((y % 12) as u32) >> (x % 6)) & 1 == 1;
        let color = if y < 40 {
            [50, 45, 40]
        } else if ink {
            [55, 40, 25]
        } else {
            [244, 244, 244]
        };
        pixel.copy_from_slice(&[color[0], color[1], color[2], 255]);
    }
    pixels
}

fn draw(frame: &mut [u8], base: &[u8], scene: &str, n: usize) {
    frame.copy_from_slice(base);
    if scene == "scroll" {
        let offset = (n * 4 % HEIGHT) * WIDTH * 4;
        let end = base.len() - offset;
        frame[..end].copy_from_slice(&base[offset..]);
        frame[end..].copy_from_slice(&base[..offset]);
    } else if scene == "local-motion" || scene == "full-motion" {
        let (width, height) = if scene == "local-motion" {
            (640, 360)
        } else {
            (WIDTH, HEIGHT)
        };
        for y in 0..height {
            for x in 0..width {
                let (a, b) = (x + n * 5, y + n * 3);
                let i = (y * WIDTH + x) * 4;
                frame[i..i + 4].copy_from_slice(&[
                    ((a + b) % 256) as u8,
                    (((a / 16) ^ (b / 16)) * 19 % 256) as u8,
                    ((a * 3 + b * 2) % 256) as u8,
                    255,
                ]);
            }
        }
    }
}

fn run(
    codec: CodecId,
    scene: &str,
    dedup: bool,
    intra: bool,
    output: &Path,
) -> Result<(), Box<dyn Error>> {
    let base = desktop();
    let mut frame = base.clone();
    let mut last = None;
    let mut encoder = VideoEncoder::new(codec, WIDTH, HEIGHT, 4000, FPS)?;
    let mut decoder = None;
    let mut bytes = Vec::new();
    let mut key_bytes = 0;
    let mut keys = 0;
    let mut decoded = 0;
    let mut encoded = 0;
    let mut timings = Vec::new();
    let mut squared_error = 0f64;
    let mut compared = 0usize;
    for n in 0..FRAMES {
        if scene == "static-probe" && n % usize::from(FPS) != 0 {
            continue;
        }
        draw(&mut frame, &base, scene, n);
        if dedup && last.as_deref() == Some(frame.as_slice()) {
            continue;
        }
        if intra {
            encoder.request_keyframe();
        }
        let now = Instant::now();
        let packets = encoder.encode_bgra(&frame, n as i64 * 1_000_000 / i64::from(FPS))?;
        timings.push(now.elapsed().as_secs_f64() * 1000.);
        if !packets.is_empty() {
            last = Some(frame.clone());
        }
        for packet in packets {
            if decoder.is_none() {
                decoder = Some(VideoDecoder::new(
                    codec,
                    WIDTH,
                    HEIGHT,
                    encoder.parameter_sets().ok_or("no parameters")?,
                )?);
            }
            encoded += 1;
            if packet.keyframe {
                keys += 1;
                key_bytes += packet.data.len();
            }
            bytes.extend_from_slice(&packet.data);
            let decoder = decoder.as_ref().unwrap();
            decoder.decode_annexb(&packet.data, packet.pts_us)?;
            let deadline = Instant::now() + Duration::from_secs(3);
            let decoded_frame = loop {
                if let Some(decoded_frame) = decoder.try_recv_decoded() {
                    break decoded_frame;
                }
                if Instant::now() >= deadline {
                    return Err("decode timeout".into());
                }
                std::thread::sleep(Duration::from_micros(100));
            };
            assert_eq!(decoded_frame.pts_us, packet.pts_us);
            decoded += 1;
            if n % 30 == 0 {
                for (actual, expected) in decoded_frame
                    .data
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(frame.as_chunks::<4>().0)
                {
                    for channel in 0..3 {
                        let delta = f64::from(actual[channel]) - f64::from(expected[channel]);
                        squared_error += delta * delta;
                        compared += 1;
                    }
                }
            }
        }
    }
    assert_eq!(decoded, encoded);
    let expected = if scene == "static-probe" {
        FRAMES / usize::from(FPS)
    } else if dedup && scene == "static" {
        1
    } else {
        FRAMES
    };
    assert_eq!(encoded, expected, "unexpected encoder drops");
    timings.sort_by(f64::total_cmp);
    let name = format!("{codec:?}-{scene}-dedup{dedup}-intra{intra}");
    let mut file = std::fs::File::create(output.join(format!("{name}.annexb")))?;
    file.write_all(&bytes)?;
    let psnr = 10. * (255f64.powi(2) / (squared_error / compared as f64).max(1e-9)).log10();
    println!(
        "{codec:?},{scene},{dedup},{intra},{FRAMES},{encoded},{},{},{keys},{key_bytes},{:.6},{:.3},{:.3},{:.2}",
        FRAMES - encoded,
        bytes.len(),
        bytes.len() as f64 * 8. / (FRAMES as f64 / f64::from(FPS)) / 1e6,
        timings[timings.len() / 2],
        timings[timings.len() * 95 / 100],
        psnr
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let output = std::env::args()
        .nth(1)
        .ok_or("provide output directory for synthetic bitstreams")?;
    let output = Path::new(&output);
    std::fs::create_dir_all(output)?;
    println!(
        "codec,scene,dedup,intra,captured,encoded,skipped,bytes,keyframes,key_bytes,mbps_at_30fps,encode_p50_ms,encode_p95_ms,sampled_bgr_psnr_db"
    );
    for codec in [CodecId::H264, CodecId::Hevc] {
        for scene in ["static", "local-motion", "scroll", "full-motion"] {
            run(codec, scene, true, false, output)?;
        }
        run(codec, "static", false, false, output)?;
        run(codec, "local-motion", true, true, output)?;
        run(codec, "static-probe", false, false, output)?;
        run(codec, "static-probe", false, true, output)?;
    }
    Ok(())
}
