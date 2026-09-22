//! Round-trip a synthetic 1080p UI at representative adaptive budgets.
//! Generate source.bgra with scripts/readability_fixture.swift first.
//! Output PPMs preserve every decoded pixel for independent visual/OCR review.
use removent_media_codec::{VideoDecoder, VideoEncoder};
use removent_proto::CodecId;
use std::{
    error::Error,
    io::Write,
    path::Path,
    time::{Duration, Instant},
};

fn ppm(path: &Path, bgra: &[u8]) -> Result<(), Box<dyn Error>> {
    let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
    file.write_all(b"P6\n1920 1080\n255\n")?;
    for p in bgra.as_chunks::<4>().0 {
        file.write_all(&[p[2], p[1], p[0]])?;
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let directory = std::env::args().nth(1).ok_or("provide fixture directory")?;
    let directory = Path::new(&directory);
    let source = std::fs::read(directory.join("source.bgra"))?;
    assert_eq!(source.len(), 1920 * 1080 * 4);
    println!(
        "codec,kbps,fps,quantizer_bound,encoded,dropped,first_frame_bytes,payload_kbps,encode_p95_ms,rgb_psnr_db"
    );
    for codec in [CodecId::H264, CodecId::Hevc] {
        for (kbps, fps) in [(8000, 30), (2000, 12), (332, 2)] {
            let mut encoder = VideoEncoder::new(codec, 1920, 1080, kbps, fps)?;
            let mut decoder = None;
            let mut bytes = 0;
            let mut decoded = None;
            let mut encoded = 0;
            let mut first_frame_bytes = 0;
            let mut times = Vec::new();
            // The first frame contains the full text; a moving bottom strip
            // exercises inter prediction without replacing that text.
            for n in 0..30 {
                let mut raw = source.clone();
                for y in 940..1080 {
                    for x in 0..1920 {
                        let i = (y * 1920 + x) * 4;
                        raw[i..i + 3].copy_from_slice(&[
                            ((x + n * 10) % 256) as u8,
                            (y % 256) as u8,
                            100,
                        ]);
                    }
                }
                let start = Instant::now();
                let packets = encoder.encode_bgra(&raw, n as i64 * 1_000_000 / i64::from(fps))?;
                times.push(start.elapsed().as_secs_f64() * 1000.);
                for packet in packets {
                    bytes += packet.data.len();
                    encoded += 1;
                    if encoded == 1 {
                        first_frame_bytes = packet.data.len();
                    }
                    if decoder.is_none() {
                        decoder = Some(VideoDecoder::new(
                            codec,
                            1920,
                            1080,
                            encoder.parameter_sets().ok_or("parameters missing")?,
                        )?);
                    }
                    let decoder = decoder.as_ref().unwrap();
                    decoder.decode_annexb(&packet.data, packet.pts_us)?;
                    let deadline = Instant::now() + Duration::from_secs(3);
                    loop {
                        if let Some(frame) = decoder.try_recv_decoded() {
                            if encoded == 1 {
                                ppm(
                                    &directory.join(format!("{codec:?}-{kbps}-{fps}-first.ppm")),
                                    &frame.data,
                                )?;
                            }
                            decoded = Some(frame.data);
                            break;
                        }
                        if Instant::now() >= deadline {
                            return Err("decode deadline".into());
                        }
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
            }
            let decoded = decoded.ok_or("no decoded output")?;
            let name = format!("{codec:?}-{kbps}-{fps}");
            ppm(&directory.join(format!("{name}.ppm")), &decoded)?;
            let mut error = 0.;
            let mut count = 0;
            // Exclude the animated strip from the still-UI comparison.
            for (actual, expected) in decoded[..940 * 1920 * 4]
                .as_chunks::<4>()
                .0
                .iter()
                .zip(source.as_chunks::<4>().0)
            {
                for c in 0..3 {
                    error += (f64::from(actual[c]) - f64::from(expected[c])).powi(2);
                    count += 1;
                }
            }
            let psnr = 10. * (255f64.powi(2) / (error / f64::from(count)).max(1e-9)).log10();
            times.sort_by(f64::total_cmp);
            println!(
                "{codec:?},{kbps},{fps},{},{encoded},{},{first_frame_bytes},{:.2},{:.2},{psnr:.2}",
                encoder.quality_ceiling_supported(),
                30 - encoded,
                bytes as f64 * 8. / (30. / f64::from(fps)) / 1000.,
                times[28]
            );
        }
    }
    Ok(())
}
