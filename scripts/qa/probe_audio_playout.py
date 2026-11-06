#!/usr/bin/env python3
"""Observe the production packet sequencer under a deterministic delivery stall.

Requires Python 3 and rustc. Compiles the actual jitter.rs in a temporary directory;
the only imported protocol constant comes from constants.rs. No audio device,
network, Tokio scheduling, codec, or production application is exercised. The
isolated sequencer probe calls pop once per 10 ms. Production decoding drains
bounded batches on arrival and also wakes periodically for concealment.
This is a diagnostic probe, not an assertion that the current behavior is desired.
"""

import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[2]
JITTER = ROOT / "packages/client/src/jitter.rs"
CONSTANTS = ROOT / "packages/proto/src/constants.rs"


def main():
    flags = re.search(
        r"pub mod audio_flags \{[^}]*\}", CONSTANTS.read_text(), re.DOTALL
    )
    if flags is None:
        raise RuntimeError("audio_flags module not found")
    harness = (
        '#![allow(dead_code)]\nextern crate self as removent_proto;\n'
        + flags.group(0)
        + '\n#[path = r#"'
        + str(JITTER)
        + '"#]\nmod jitter;\n'
        + r'''
use jitter::{AudioPacketIn, JitterBuffer, PopOutcome};

fn packet(seq: u16, pts_us: i64) -> AudioPacketIn {
    AudioPacketIn { seq, pts_us, flags: 0, payload: vec![1] }
}
fn main() {
    use std::time::{Duration, Instant};
    let t = Instant::now();
    let mut buffer = JitterBuffer::new(60);
    buffer.push_at(packet(0, 0), t);
    buffer.pop_at(t);
    for tick in 1..12 { buffer.pop_at(t + Duration::from_millis(tick * 10)); }
    for seq in 1..=12 {
        buffer.push_at(packet(seq, i64::from(seq) * 10_000), t + Duration::from_millis(120));
    }
    let first_pts = match buffer.pop_at(t + Duration::from_millis(120)) {
        PopOutcome::Packet(p) => p.pts_us,
        result => panic!("expected recovery packet, got {result:?}"),
    };
    let mut last_pts = first_pts;
    for seq in 13..=112 {
        let now = t + Duration::from_millis(u64::from(seq) * 10);
        buffer.push_at(packet(seq, i64::from(seq) * 10_000), now);
        if let PopOutcome::Packet(p) = buffer.pop_at(now) { last_pts = p.pts_us; }
    }
    let mut cap = JitterBuffer::new(60);
    for seq in 0..25 { cap.push_at(packet(seq, i64::from(seq) * 10_000), t); }
    let mut gap = JitterBuffer::new(60);
    gap.push_at(packet(0, 0), t); gap.pop_at(t);
    gap.push_at(packet(1, 40_000), t + Duration::from_millis(40));
    let concealed = matches!(gap.pop_at(t + Duration::from_millis(40)), PopOutcome::Plc(_));
    println!("{{\"age_at_recovery_ms\":{},\"age_after_100_normal_ticks_ms\":{},\"remaining_depth_ms\":{},\"configured_depth_ms\":60,\"depth_after_25_packet_burst_ms\":{},\"burst_evictions\":{},\"timestamp_gap_generated_plc\":{concealed}}}",
        (120_000 - first_pts) / 1000, (1_120_000 - last_pts) / 1000,
        buffer.depth_ms(), cap.depth_ms(), cap.late_dropped);
}

'''
    )
    with tempfile.TemporaryDirectory(prefix="removent-audio-playout-") as directory:
        source = Path(directory) / "probe.rs"
        binary = Path(directory) / "probe"
        source.write_text(harness)
        subprocess.run(
            ["rustc", "--edition=2024", "-O", str(source), "-o", str(binary)],
            check=True,
        )
        result = json.loads(subprocess.check_output([str(binary)], text=True))
    print(json.dumps({
        "scope": "production JitterBuffer with a simulated 10 ms playout clock",
        "jitter_sha256": hashlib.sha256(JITTER.read_bytes()).hexdigest(),
        "observations": result,
    }, indent=2))


if __name__ == "__main__":
    main()
