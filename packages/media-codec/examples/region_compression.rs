//! Compare whole-frame HEVC with an experimental stable-tile side channel.
//! cargo run --release -p removent-media-codec --example region_compression -- [all|mixed|static|scroll|game|late-edit] [video_kbps] [frames] [fps] [width] [height] [link_kbps]
//! REMOVENT_REGION_TRACE_DIR writes application-byte traces. REMOVENT_REGION_REPLAY_DIR also saves decoded display updates.
#[path = "region_compression/fixture.rs"]
mod fixture;
#[path = "region_compression/pacing.rs"]
mod pacing;
#[path = "region_compression/tiles.rs"]
mod tiles;

use fixture::{Fixture, Scene};
use removent_media_codec::{VideoDecoder, VideoEncoder};
use removent_proto::{
    CodecId, VIDEO_HEADER_LEN, VideoFrameHeader, video_flags, write_video_header,
};
use std::{
    error::Error,
    fs::{self, File},
    io::{BufWriter, Write},
    path::Path,
    time::{Duration, Instant},
};
use tiles::{Grid, Receiver, Sender};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
type Rect = (u32, u32, u32, u32);

// Experimental mode only. Attached to the video whose pixels supersede cached tiles.
const RESET_PATCHES: u8 = 1 << 7;

#[derive(Default)]
struct Quality {
    bgr_sse: u64,
    channels: u64,
    edge_abs_error: u64,
    edge_channels: u64,
}

impl Quality {
    fn observe(&mut self, source: &[u8], shown: &[u8], width: usize, area: Rect) {
        let (x, y, w, h) = area;
        for row in y..y + h {
            let start = (row as usize * width + x as usize) * 4;
            let end = start + w as usize * 4;
            let mut previous = None;
            for (expected, actual) in source[start..end]
                .as_chunks::<4>()
                .0
                .iter()
                .zip(shown[start..end].as_chunks::<4>().0.iter())
            {
                let luma =
                    (i32::from(expected[0]) + 2 * i32::from(expected[1]) + i32::from(expected[2]))
                        / 4;
                let edge = previous.is_some_and(|v: i32| (luma - v).abs() >= 32);
                previous = Some(luma);
                for c in 0..3 {
                    let delta = u64::from(expected[c].abs_diff(actual[c]));
                    self.bgr_sse += delta * delta;
                    if edge {
                        self.edge_abs_error += delta;
                    }
                }
                self.channels += 3;
                if edge {
                    self.edge_channels += 3;
                }
            }
        }
    }

    fn psnr(&self) -> String {
        if self.channels == 0 {
            return String::new();
        }
        format!(
            "{:.3}",
            10.0 * (255f64.powi(2) * self.channels as f64 / self.bgr_sse as f64).log10()
        )
    }

    fn edge_mae(&self) -> String {
        if self.edge_channels == 0 {
            return String::new();
        }
        format!(
            "{:.3}",
            self.edge_abs_error as f64 / self.edge_channels as f64
        )
    }
}

struct Replay {
    images: Option<(BufWriter<File>, BufWriter<File>)>,
    trace: BufWriter<File>,
    media: BufWriter<File>,
}

impl Replay {
    fn new(dir: &Path, name: &str, save_images: bool) -> Result<Self> {
        fs::create_dir_all(dir)?;
        let path = dir.join(name);
        let mut trace = BufWriter::new(File::create(path.with_extension("csv"))?);
        writeln!(
            trace,
            "pts_us,video_bytes,patch_bytes,patch_budget_bytes,installed,invalidated,reset_patches"
        )?;
        let images = if save_images {
            Some((
                BufWriter::new(File::create(path.with_extension("bgra"))?),
                BufWriter::new(File::create(path.with_extension("pts"))?),
            ))
        } else {
            None
        };
        Ok(Self {
            images,
            trace,
            media: BufWriter::new(File::create(path.with_extension("media"))?),
        })
    }

    fn finish(&mut self) -> Result<()> {
        if let Some((pixels, pts)) = &mut self.images {
            pixels.flush()?;
            pts.flush()?;
        }
        self.trace.flush()?;
        self.media.flush()?;
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct Settings {
    grid: Grid,
    frames: u32,
    fps: u8,
    kbps: u32,
    link_kbps: u32,
}

#[derive(Clone, Copy, PartialEq)]
enum Refinement {
    Off,
    Original,
    Paced,
}

struct LinkChange {
    at_us: i64,
    kbps: u32,
    blocked: bool,
}

// A recorded change takes effect at its own timestamp, even between video frames.
fn link_changes() -> Result<Vec<LinkChange>> {
    let Some(path) = std::env::var_os("REMOVENT_REGION_LINK_TRACE") else {
        return Ok(Vec::new());
    };
    let trace = fs::read_to_string(path)?;
    let mut lines = trace.lines();
    if lines.next() != Some("pts_us,kbps,blocked") {
        return Err("expected pts_us,kbps,blocked".into());
    }
    let mut changes = Vec::new();
    for line in lines {
        let mut row = line.split(',');
        let at_us = row.next().ok_or("missing time")?.parse()?;
        let kbps = row.next().ok_or("missing rate")?.parse()?;
        let blocked = row.next().ok_or("missing blocked flag")?.parse()?;
        if row.next().is_some()
            || at_us < 0
            || changes
                .last()
                .is_some_and(|prev: &LinkChange| prev.at_us >= at_us)
        {
            return Err("expected increasing nonnegative timestamps and three columns".into());
        }
        changes.push(LinkChange {
            at_us,
            kbps,
            blocked,
        });
    }
    Ok(changes)
}

impl Settings {
    fn encoder(self) -> Result<VideoEncoder> {
        Ok(VideoEncoder::new(
            CodecId::Hevc,
            self.grid.width,
            self.grid.height,
            self.kbps,
            self.fps,
        )?)
    }

    fn pts(self, n: u32) -> i64 {
        i64::from(n) * 1_000_000 / i64::from(self.fps)
    }
}

#[derive(Default)]
struct Stats {
    video: usize,
    patch: usize,
    peak_patch: usize,
    decoded: usize,
    skipped: usize,
    display_updates: usize,
    installs: usize,
    clears: usize,
    encode_us: Vec<f64>,
    sender_us: Vec<f64>,
    receiver_us: Vec<f64>,
    total_us: Vec<f64>,
    text: Quality,
    motion: Quality,
}

fn micros(from: Instant) -> f64 {
    from.elapsed().as_secs_f64() * 1e6
}

fn pct_ms(samples: &mut [f64], fraction: f64) -> f64 {
    if samples.is_empty() {
        return 0.;
    }
    samples.sort_unstable_by(f64::total_cmp);
    samples[(samples.len() as f64 * fraction).ceil() as usize - 1] / 1000.
}

impl Stats {
    fn bytes(&self) -> usize {
        self.video + self.patch
    }

    fn report(&mut self, name: &str, variant: &str, config: Settings, target: usize) {
        let total = self.bytes();
        let kbps = 8. * total as f64 * f64::from(config.fps) / (1000. * f64::from(config.frames));
        let error = if target == 0 {
            String::new()
        } else {
            format!("{:.3}", 100. * (total as f64 / target as f64 - 1.))
        };
        println!(
            "{name},{variant},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{kbps:.3},{error},{},{},{},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{}",
            config.grid.width,
            config.grid.height,
            config.fps,
            config.frames,
            config.kbps,
            self.decoded,
            self.skipped,
            self.display_updates,
            self.installs,
            self.clears,
            self.video,
            self.patch,
            self.peak_patch,
            total,
            self.text.psnr(),
            self.text.edge_mae(),
            self.motion.psnr(),
            pct_ms(&mut self.encode_us, 0.95),
            pct_ms(&mut self.sender_us, 0.95),
            pct_ms(&mut self.receiver_us, 0.95),
            pct_ms(&mut self.total_us, 0.50),
            pct_ms(&mut self.total_us, 0.95),
            pct_ms(&mut self.total_us, 0.99),
            config.link_kbps
        );
    }
}

fn decode(decoder: &VideoDecoder, packet: &[u8], pts: i64) -> Result<Vec<u8>> {
    decoder.decode_annexb(packet, pts)?;
    let start = Instant::now();
    loop {
        if let Some(output) = decoder.try_recv_decoded() {
            if output.pts_us != pts {
                return Err("unexpected decoded timestamp".into());
            }
            return Ok(output.data);
        }
        if start.elapsed() > Duration::from_secs(3) {
            return Err("decode timed out".into());
        }
        std::thread::sleep(Duration::from_micros(100));
    }
}

fn measure(
    fixture: &Fixture,
    config: Settings,
    refinement: Refinement,
    mut updates: &[LinkChange],
    mut replay: Option<Replay>,
) -> Result<Stats> {
    let mut encoder = config.encoder()?;
    let mut decoder = None;
    let mut previous: Option<Vec<u8>> = None;
    let mut base = Vec::new();
    let mut base_pts = -1;
    let mut composited = Vec::new();
    let mut sender = Sender::new(config.grid);
    let mut receiver = Receiver::new(config.grid);
    let mut pacer = pacing::PatchPacer::new(config.link_kbps);
    let mut stats = Stats::default();
    for n in 0..config.frames {
        let source = fixture.render(n);
        let pts = config.pts(n);
        let start = Instant::now();
        let same = previous.as_deref() == Some(source.as_slice());
        let mut video_bytes = 0;
        let mut packets = Vec::new();
        let mut batch = Vec::new();
        let mut reset_patches = false;
        if same {
            stats.skipped += 1;
        } else {
            let encode = Instant::now();
            packets = encoder.encode_bgra(&source, pts)?;
            stats.encode_us.push(micros(encode));
            for packet in &packets {
                if packet.pts_us != pts {
                    return Err("benchmark expects low-delay HEVC".into());
                }
                if decoder.is_none() {
                    decoder = Some(VideoDecoder::new(
                        CodecId::Hevc,
                        config.grid.width,
                        config.grid.height,
                        encoder.parameter_sets().ok_or("no HEVC parameter sets")?,
                    )?);
                }
                base = decode(decoder.as_ref().unwrap(), &packet.data, pts)?;
                base_pts = pts;
                video_bytes += packet.data.len() + VIDEO_HEADER_LEN;
                stats.decoded += 1;
            }
        }
        // Only advance the overlay if the corresponding base image was delivered
        // or if it is identical to the last successful source. A dropped encode
        // must not cause overlays on the receiver's previous base to disappear.
        let delivered = same || video_bytes != 0;
        let before = (receiver.installs, receiver.invalidations);
        let budget = if refinement == Refinement::Paced {
            while let Some((change, rest)) = updates.split_first() {
                if change.at_us > pts {
                    break;
                }
                pacer.set_link(change.at_us, change.kbps, change.blocked);
                updates = rest;
            }
            Some(pacer.start_frame(pts, config.pts(n + 1), video_bytes))
        } else {
            None
        };
        if refinement != Refinement::Off && delivered {
            let send_start = Instant::now();
            batch = if let Some(bytes) = budget {
                let batch =
                    sender.observe_budgeted(&source, previous.as_deref(), pts, base_pts, bytes)?;
                pacer.charge_patch(batch.len());
                batch
            } else {
                sender.observe_unpaced_reference(&source, previous.as_deref(), pts, base_pts)?
            };
            stats.sender_us.push(micros(send_start));
            let receive_start = Instant::now();
            reset_patches = sender.take_reset_flag();
            if reset_patches {
                if video_bytes == 0 {
                    return Err("a tile reset must accompany a new video frame".into());
                }
                receiver.reset_patches();
            }
            if !batch.is_empty() {
                receiver.apply(&batch, pts, base_pts)?;
            }
            if receiver.has_patches()
                && (video_bytes != 0 || before != (receiver.installs, receiver.invalidations))
            {
                composited.clone_from(&base);
                receiver.composite(&mut composited);
            }
            stats.receiver_us.push(micros(receive_start));
        }
        stats.total_us.push(micros(start));
        stats.video += video_bytes;
        stats.patch += batch.len();
        stats.peak_patch = stats.peak_patch.max(batch.len());
        let visible = if receiver.has_patches() {
            &composited
        } else {
            &base
        };
        let installed = receiver.installs - before.0;
        let invalidated = receiver.invalidations - before.1;
        let event = video_bytes != 0 || installed + invalidated != 0;
        if let Some(replay) = replay.as_mut() {
            for (i, packet) in packets.iter().enumerate() {
                let header = VideoFrameHeader {
                    frame_id: (stats.decoded - packets.len() + i + 1) as u64,
                    pts_us: packet.pts_us,
                    flags: (if packet.keyframe {
                        video_flags::KEYFRAME
                    } else {
                        0
                    }) | (if reset_patches { RESET_PATCHES } else { 0 }),
                    codec: CodecId::Hevc,
                    width: config.grid.width as u16,
                    height: config.grid.height as u16,
                    payload_len: packet.data.len() as u32,
                };
                let mut bytes = Vec::with_capacity(VIDEO_HEADER_LEN);
                write_video_header(&mut bytes, &header);
                replay.media.write_all(&bytes)?;
                replay.media.write_all(&packet.data)?;
            }
            replay.media.write_all(&batch)?;
            let limit = budget.map(|bytes| bytes.to_string()).unwrap_or_default();
            writeln!(
                replay.trace,
                "{pts},{video_bytes},{},{limit},{installed},{invalidated},{}",
                batch.len(),
                u8::from(reset_patches)
            )?;
            if event && let Some((pixels, timestamps)) = &mut replay.images {
                writeln!(timestamps, "{pts}")?;
                pixels.write_all(visible)?;
            }
        }
        if event {
            stats.display_updates += 1;
        }
        // Sample the image held at every source tick, including skipped/dropped
        // ticks, so a codec cannot improve quality by skipping difficult frames.
        if !visible.is_empty() {
            if visible.len() != source.len() {
                return Err("decoded dimensions differ".into());
            }
            if let Some(text) = fixture.text_rect() {
                stats
                    .text
                    .observe(&source, visible, config.grid.width, text);
            }
            stats
                .motion
                .observe(&source, visible, config.grid.width, fixture.motion_rect());
        }
        if delivered {
            previous = Some(source);
        }
    }
    if stats.decoded == 0 {
        return Err("no decoded frames".into());
    }
    stats.installs = receiver.installs;
    stats.clears = receiver.invalidations;
    if let Some(replay) = replay.as_mut() {
        replay.finish()?;
    }
    Ok(stats)
}

// Match measured *bytes*, including the original frame header and all patch
// framing. The VT quality ceiling can make targets unattainable on still UI.
// Return the best observed rate and print the residual of the fresh final run.
fn calibrate(fixture: &Fixture, config: Settings, initial: usize, target: usize) -> Result<u32> {
    let mut best = (initial.abs_diff(target), config.kbps);
    let mut low = 0;
    let mut high = None;
    let mut rate = config.kbps;
    let mut size = initial;
    for _ in 0..8 {
        if best.0 as f64 <= 0.01 * target as f64 {
            break;
        }
        if size < target {
            low = rate;
        } else {
            high = Some(rate);
        }
        let next = high.map_or_else(
            || rate.saturating_mul(2).min(100_000),
            |h| low + (h - low) / 2,
        );
        if next == rate || next == 0 {
            break;
        }
        rate = next;
        let mut encoder = Settings {
            kbps: rate,
            ..config
        }
        .encoder()?;
        let mut previous: Option<Vec<u8>> = None;
        size = 0;
        for n in 0..config.frames {
            let source = fixture.render(n);
            if previous.as_deref() == Some(source.as_slice()) {
                continue;
            }
            let packets = encoder.encode_bgra(&source, config.pts(n))?;
            if !packets.is_empty() {
                previous = Some(source);
            }
            size += packets
                .iter()
                .map(|p| p.data.len() + VIDEO_HEADER_LEN)
                .sum::<usize>();
        }
        if size.abs_diff(target) < best.0 {
            best = (size.abs_diff(target), rate);
        }
    }
    Ok(best.1)
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let value = |n: usize, default: &'static str| args.get(n).map_or(default, String::as_str);
    let selected = value(0, "all");
    let config = Settings {
        kbps: value(1, "2000").parse()?,
        link_kbps: args
            .get(6)
            .map_or(value(1, "2000"), String::as_str)
            .parse()?,
        frames: value(2, "120").parse()?,
        fps: value(3, "30").parse()?,
        grid: Grid {
            width: value(4, "1280").parse()?,
            height: value(5, "720").parse()?,
        },
    };
    if config.frames == 0
        || config.fps == 0
        || !(1..=100_000).contains(&config.kbps)
        || config.grid.width == 0
        || config.grid.height == 0
        || config.grid.width > 3840
        || config.grid.height > 2160
        || !config.grid.width.is_multiple_of(2)
        || !config.grid.height.is_multiple_of(2)
        || config.link_kbps == 0
        || args.len() > 7
    {
        return Err("invalid bitrate, dimensions, FPS or frame count".into());
    }
    let scenes = [
        ("mixed", Scene::Mixed),
        ("scroll", Scene::Scroll),
        ("game", Scene::Game),
        ("static", Scene::Static),
        ("late-edit", Scene::LateEdit),
    ];
    if selected != "all" && !scenes.iter().any(|(name, _)| *name == selected) {
        return Err("unknown scene".into());
    }
    let replay_dir = std::env::var_os("REMOVENT_REGION_REPLAY_DIR");
    let save_images = replay_dir.is_some();
    let directory = replay_dir
        .or_else(|| std::env::var_os("REMOVENT_REGION_TRACE_DIR"))
        .map(std::path::PathBuf::from);
    let link_changes = link_changes()?;
    println!(
        "scene,variant,width,height,fps,ticks,target_video_kbps,decoded,unchanged_ticks,display_updates,patch_installs,patch_invalidations,video_bytes,patch_bytes,peak_patch_bytes,total_bytes,total_kbps,delta_vs_hybrid_pct,text_psnr_db,text_edge_mae,motion_psnr_db,encode_p95_ms,patch_send_p95_ms,patch_receive_compose_p95_ms,pipeline_p50_ms,pipeline_p95_ms,pipeline_p99_ms,link_kbps"
    );
    for (name, scene) in scenes {
        if selected == "all" && scene == Scene::LateEdit {
            continue;
        }
        if selected != "all" && selected != name {
            continue;
        }
        let fixture = Fixture::new(scene, config.grid.width as u32, config.grid.height as u32)?;
        let replay = |variant: &str| {
            directory
                .as_ref()
                .map(|dir| Replay::new(dir, &format!("{name}-{variant}"), save_images))
                .transpose()
        };
        let mut baseline = measure(&fixture, config, Refinement::Off, &[], replay("baseline")?)?;
        let mut hybrid = measure(
            &fixture,
            config,
            Refinement::Original,
            &[],
            replay("hybrid")?,
        )?;
        let target = hybrid.bytes();
        baseline.report(name, "baseline", config, target);
        hybrid.report(name, "hybrid", config, target);
        let matched = Settings {
            kbps: calibrate(&fixture, config, baseline.bytes(), target)?,
            ..config
        };
        measure(&fixture, matched, Refinement::Off, &[], replay("matched")?)?
            .report(name, "matched", matched, target);
        measure(
            &fixture,
            config,
            Refinement::Paced,
            &link_changes,
            replay("paced")?,
        )?
        .report(name, "paced", config, target);
    }
    Ok(())
}
