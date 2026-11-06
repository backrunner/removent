//! Five seconds of silence through the production CPAL callback. This measures
//! callback delivery and OS-reported timing, not acoustic/source-to-ear latency.
#[path = "../src/audio.rs"]
mod audio;
use removent_client::audio::DecodedAudio;
use std::time::{Duration, Instant};
fn main() -> anyhow::Result<()> {
    let player = audio::AudioPlayer::new().map_err(anyhow::Error::msg)?;
    let start = Instant::now();
    for period in 0..500 {
        let at = start + Duration::from_millis(period * 10);
        std::thread::sleep(at.saturating_duration_since(Instant::now()));
        player.push(DecodedAudio {
            samples: vec![0; 960],
            pts_us: period as i64 * 10_000,
            source_at: at,
            discontinuity: false,
        });
    }
    let stats = player.stats();
    println!("{stats:?}");
    anyhow::ensure!(
        stats.rendered_frames > 48_000 * 4,
        "output callback did not consume four seconds"
    );
    anyhow::ensure!(
        stats.dropped_blocks == 0 && stats.underruns == 0,
        "device continuity failed"
    );
    player.clear();
    Ok(())
}
