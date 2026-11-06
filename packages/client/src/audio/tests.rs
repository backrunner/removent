use super::*;

fn block(pts_us: i64, source_at: Instant) -> DecodedAudio {
    DecodedAudio {
        samples: [12_000, -12_000].repeat(BLOCK_FRAMES),
        pts_us,
        source_at,
        discontinuity: false,
    }
}

#[test]
fn stalled_device_discards_stale_sound_and_resumes_fresh_audio() {
    let q = Arc::new(AudioQueue::default());
    let mut renderer = AudioRenderer::new(q.clone());
    let t = Instant::now();
    for i in 0..6 {
        q.push(block(i * 10_000, t));
    }
    renderer.render(480, RATE, t + Duration::from_millis(200), |_, frame| {
        assert_eq!(frame, [0.; 2])
    });
    assert_eq!(q.stats().dropped_blocks, 6);
    q.push(block(200_000, t + Duration::from_millis(200)));
    q.push(block(210_000, t + Duration::from_millis(210)));
    renderer.render(240, RATE, t + Duration::from_millis(210), |_, _| {});
    let mut loud = false;
    renderer.render(480, RATE, t + Duration::from_millis(240), |_, frame| {
        loud |= frame[0] > 0.3;
        assert!((frame[0] + frame[1]).abs() < 0.0001);
    });
    assert!(loud);
}

#[test]
fn device_clock_drift_does_not_accumulate_a_queue_or_drop_audio() {
    for source_ppm in [-150., 150.] {
        let q = Arc::new(AudioQueue::default());
        let mut r = AudioRenderer::new(q.clone());
        let t = Instant::now();
        let mut next_source = 0.;
        let mut seq = 0;
        // Five minutes of 5 ms device callbacks, with independent source clock.
        for tick in 0..60_000 {
            let elapsed = tick as f64 * 0.005;
            let now = t + Duration::from_secs_f64(elapsed);
            while next_source <= elapsed {
                q.push(block(seq * 10_000, now));
                seq += 1;
                next_source += 0.01 / (1. + source_ppm / 1_000_000.);
            }
            r.render(240, RATE, now, |_, pair| {
                assert!(pair.iter().all(|x| x.is_finite()))
            });
        }
        assert_eq!(q.stats().dropped_blocks, 0, "{source_ppm} ppm");
        assert_eq!(q.stats().underruns, 0, "{source_ppm} ppm");
        assert!(q.stats().queued_frames <= TARGET_FRAMES * 2);
    }
}

#[test]
fn clear_releases_retained_audio_and_fades_without_a_click() {
    let q = Arc::new(AudioQueue::default());
    let mut r = AudioRenderer::new(q.clone());
    let t = Instant::now();
    q.push(block(0, t));
    q.push(block(10_000, t));
    r.render(480, RATE, t, |_, _| {});
    r.render(480, RATE, t + Duration::from_millis(10), |_, _| {});
    q.clear();
    let mut previous = r.last;
    r.render(480, RATE, t, |_, pair| {
        assert!((pair[0] - previous[0]).abs() < 0.01);
        previous = pair;
    });
    assert_eq!(previous, [0.; 2]);
}

#[test]
fn expiring_a_partial_block_fades_even_if_next_pts_is_contiguous() {
    let q = Arc::new(AudioQueue::default());
    let mut r = AudioRenderer::new(q.clone());
    let t = Instant::now();
    q.push(block(0, t));
    let mut next = block(10_000, t + Duration::from_millis(10));
    next.samples.iter_mut().for_each(|v| *v = -*v);
    q.push(next);
    r.render(240, RATE, t, |_, _| {});
    r.render(240, RATE, t + Duration::from_millis(30), |_, _| {});
    let mut previous = r.last[0];
    r.render(240, RATE, t + Duration::from_millis(90), |_, pair| {
        assert!((pair[0] - previous).abs() < 0.01);
        previous = pair[0];
    });
    assert_eq!(q.stats().dropped_blocks, 1);
    assert_eq!(q.stats().discontinuities, 1);
}

#[test]
fn regular_capture_batches_do_not_cause_device_underruns() {
    for (periods, output_rate) in [2, 4, 6]
        .into_iter()
        .flat_map(|periods| [44_100u32, RATE].map(|rate| (periods, rate)))
    {
        let q = Arc::new(AudioQueue::default());
        let mut r = AudioRenderer::new(q.clone());
        let t = Instant::now() + Duration::from_secs(1);
        let mut seq = 0;
        let output_frames = output_rate as usize / 200;
        let mut next_capture_us = 0;
        for tick in 0..12_000 {
            let elapsed_us = tick * output_frames as u64 * 1_000_000 / u64::from(output_rate);
            let now = t + Duration::from_micros(elapsed_us);
            while next_capture_us <= elapsed_us {
                let captured = t + Duration::from_micros(next_capture_us);
                for i in 0..periods {
                    q.push(block(
                        seq * 10_000,
                        captured - Duration::from_millis((periods - i - 1) * 10),
                    ));
                    seq += 1;
                }
                next_capture_us += periods * 10_000;
            }
            r.render(output_frames, output_rate, now, |_, _| {});
            assert_eq!(
                q.stats().underruns,
                0,
                "batch={periods} rate={output_rate} tick={tick} ratio={} water={} batch_frames={} dropped={}",
                r.rate_ratio,
                r.last_queued,
                r.batch_frames,
                q.stats().dropped_blocks
            );
        }
        assert_eq!(q.stats().underruns, 0, "{periods} period capture batch");
        assert_eq!(
            q.stats().dropped_blocks,
            0,
            "{periods} period capture batch"
        );
    }
}

#[test]
fn recorded_capture_arrivals_survive_independent_decode_and_device_clocks() {
    use crate::jitter::{AudioPacketIn, JitterBuffer, PopOutcome};

    let arrivals: Vec<(i64, u64)> =
        include_str!("../../tests/fixtures/br-gateway-audio-arrivals.csv")
            .lines()
            .filter(|line| !line.starts_with('#'))
            .map(|line| {
                let (pts, at) = line.split_once(',').unwrap();
                (pts.parse().unwrap(), at.parse().unwrap())
            })
            .collect();
    for (decode_phase, output_rate) in [0, 3_000, 7_000, 9_000]
        .into_iter()
        .flat_map(|phase| [44_100u32, RATE].map(|rate| (phase, rate)))
    {
        let output_frames = output_rate as usize / 200;
        let t = Instant::now() + Duration::from_secs(1);
        let q = Arc::new(AudioQueue::default());
        let mut renderer = AudioRenderer::new(q.clone());
        let mut jitter = JitterBuffer::new(60);
        let mut arrival = 0;
        let mut next_decode = decode_phase;
        let mut next_device = 0;
        let mut device_frames = 0u64;
        let mut arrival_wakeup = u64::MAX;
        let end = arrivals.last().unwrap().1;
        let mut startup_underruns = 0;
        while next_device <= end {
            let next_arrival = arrivals.get(arrival).map_or(u64::MAX, |row| row.1);
            let at = next_arrival
                .min(next_decode)
                .min(next_device)
                .min(arrival_wakeup);
            let now = t + Duration::from_micros(at);
            if at == next_arrival {
                jitter.push_at(
                    AudioPacketIn {
                        seq: arrival as u16,
                        pts_us: arrivals[arrival].0,
                        flags: 0,
                        payload: vec![],
                    },
                    now,
                );
                arrival += 1;
                arrival_wakeup = arrival_wakeup.min(at + 1_000);
            } else if at == next_decode || at == arrival_wakeup {
                for _ in 0..6 {
                    if jitter.pop_at(now) == PopOutcome::Wait {
                        break;
                    }
                    let (pts, source_at, discontinuity) = jitter.output_timing().unwrap();
                    let mut frame = block(pts, source_at);
                    frame.discontinuity = discontinuity;
                    q.push(frame);
                }
                if at == next_decode {
                    next_decode += 10_000;
                }
                arrival_wakeup = u64::MAX;
            } else {
                renderer.render(output_frames, output_rate, now, |_, pair| {
                    assert!(pair.iter().all(|value| value.is_finite()));
                });
                if at < 2_000_000 {
                    startup_underruns = q.stats().underruns;
                }
                // The actual 44.1 kHz device uses 220 frames, so callbacks are
                // 4.989 ms apart. Do not simulate it with a rounded 5 ms clock.
                device_frames += output_frames as u64;
                next_device = device_frames * 1_000_000 / u64::from(output_rate);
            }
        }
        let context = format!("decode phase={decode_phase}, device rate={output_rate}");
        assert_eq!(jitter.late_dropped, 0, "{context}");
        assert_eq!(jitter.concealed_frames, 0, "{context}");
        assert_eq!(q.stats().dropped_blocks, 0, "{context}");
        // The captured stream has a delay step during startup. One rebuffer is
        // allowed while settling; it must not become ongoing late drops or
        // underruns for the remaining 18 seconds.
        assert!(startup_underruns <= 1, "{context}: {startup_underruns}");
        assert_eq!(q.stats().underruns, startup_underruns, "{context}");
    }
}
