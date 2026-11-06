use super::*;

fn packet(seq: u16, pts_us: i64) -> AudioPacketIn {
    AudioPacketIn {
        seq,
        pts_us,
        flags: 0,
        payload: vec![1],
    }
}

#[test]
fn retransmission_stall_recovers_without_permanent_latency() {
    let mut buffer = JitterBuffer::new(60);
    let t = Instant::now();
    buffer.push_at(packet(0, 0), t);
    assert!(matches!(buffer.pop_at(t), PopOutcome::Packet(_)));
    for tick in 1..12 {
        buffer.pop_at(t + Duration::from_millis(tick * 10));
    }
    for seq in 1..=12 {
        buffer.push_at(
            packet(seq, i64::from(seq) * 10_000),
            t + Duration::from_millis(120),
        );
    }
    for tick in 12..112u16 {
        if tick > 12 {
            buffer.push_at(
                packet(tick, i64::from(tick) * 10_000),
                t + Duration::from_millis(u64::from(tick) * 10),
            );
        }
        match buffer.pop_at(t + Duration::from_millis(u64::from(tick) * 10)) {
            PopOutcome::Packet(p) => assert!(i64::from(tick) * 10_000 - p.pts_us <= 20_000),
            other => panic!("fresh audio must resume: {other:?}"),
        }
    }
    assert!(buffer.depth_ms() <= 20);
    assert!(buffer.late_dropped >= 9);
}

#[test]
fn capture_batches_preserve_every_sample_period_and_seq_wraps() {
    let t = Instant::now();
    let mut buffer = JitterBuffer::new(60);
    for (i, seq) in [65534, 65535, 0].into_iter().enumerate() {
        buffer.push_at(packet(seq, i as i64 * 10_000), t);
    }
    for seq in [65534, 65535, 0] {
        assert!(matches!(buffer.pop_at(t), PopOutcome::Packet(p) if p.seq == seq));
    }
}

#[test]
fn regular_capture_batches_are_not_mistaken_for_retransmission_backlog() {
    for periods in [2, 4, 6] {
        let t = Instant::now();
        let mut b = JitterBuffer::new(60);
        for batch in 0..100u16 {
            let first = batch * periods;
            let now = t + Duration::from_millis(u64::from(first) * 10);
            for seq in first..first + periods {
                b.push_at(packet(seq, i64::from(seq) * 10_000), now);
            }
            for seq in first..first + periods {
                assert!(
                    matches!(b.pop_at(now), PopOutcome::Packet(p) if p.seq == seq),
                    "{periods} period capture batch must stay continuous"
                );
            }
        }
        assert_eq!(b.late_dropped, 0);
    }
}

#[test]
fn timestamp_gap_is_concealed_even_when_sequence_is_contiguous() {
    let t = Instant::now();
    let mut b = JitterBuffer::new(60);
    b.push_at(packet(0, 0), t);
    b.pop_at(t);
    b.push_at(packet(1, 30_000), t + Duration::from_millis(30));
    assert!(matches!(
        b.pop_at(t + Duration::from_millis(30)),
        PopOutcome::Plc(_)
    ));
    assert_eq!(b.output_timing().unwrap().0, 10_000);
    assert!(matches!(
        b.pop_at(t + Duration::from_millis(40)),
        PopOutcome::Plc(_)
    ));
    assert!(
        matches!(b.pop_at(t + Duration::from_millis(50)), PopOutcome::Packet(p) if p.pts_us == 30_000)
    );
}

#[test]
fn empty_stream_concealment_is_bounded_and_new_capture_reanchors() {
    let t = Instant::now();
    let mut b = JitterBuffer::new(60);
    b.push_at(packet(0, 8_000_000), t);
    b.pop_at(t);
    for tick in 1..100 {
        b.pop_at(t + Duration::from_millis(tick * 10));
    }
    assert_eq!(b.concealed_frames, 3);
    b.push_at(packet(1, 0), t + Duration::from_secs(1));
    assert!(matches!(
        b.pop_at(t + Duration::from_secs(1)),
        PopOutcome::Packet(_)
    ));
    assert!(b.output_timing().unwrap().2);
}

#[test]
fn depth_limit_is_the_requested_limit_not_four_times_it() {
    let t = Instant::now();
    let mut b = JitterBuffer::new(60);
    for seq in 0..25 {
        b.push_at(packet(seq, i64::from(seq) * 10_000), t);
    }
    assert_eq!(b.depth_ms(), 60);
    assert_eq!(b.late_dropped, 19);
}

#[test]
fn independent_capture_clock_does_not_turn_drift_into_late_drops() {
    let t = Instant::now();
    let mut b = JitterBuffer::new(60);
    for frame in 0..30_000u64 {
        let now = t + Duration::from_micros(frame * 10_000 + frame * 15 / 10);
        b.push_at(packet(frame as u16, frame as i64 * 10_000), now);
        assert!(matches!(b.pop_at(now), PopOutcome::Packet(_)));
    }
    assert_eq!(b.late_dropped, 0);
}

#[test]
fn fractional_capture_batches_survive_an_independent_playout_clock() {
    let t = Instant::now();
    let mut b = JitterBuffer::new(60);
    let mut callback = 1u64;
    let mut next_seq = 0u16;
    let mut played = 0u16;
    // A 1024-sample callback is 21.333 ms, so 10 ms packet counts alternate.
    // Exercise empty playout ticks too, not just pop immediately after push.
    for millis in 0..10_000u64 {
        let now_us = millis * 1000;
        let now = t + Duration::from_micros(now_us);
        if now_us >= callback * 1024 * 1_000_000 / 48_000 {
            let captured_us = callback * 1024 * 1_000_000 / 48_000;
            while (u64::from(next_seq) + 1) * 10_000 <= captured_us {
                b.push_at(packet(next_seq, i64::from(next_seq) * 10_000), now);
                next_seq += 1;
            }
            callback += 1;
        }
        if millis % 10 == 3 {
            for _ in 0..6 {
                match b.pop_at(now) {
                    PopOutcome::Packet(p) => {
                        assert_eq!(p.seq, played, "continuous capture must not skip periods");
                        played += 1;
                    }
                    PopOutcome::Plc(_) => panic!("normal capture cadence must not invoke PLC"),
                    PopOutcome::Wait => break,
                }
            }
        }
    }
    assert!(played > 990);
    assert_eq!(b.late_dropped, 0);
}
