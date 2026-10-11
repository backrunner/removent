use apple_cf::cm::{CMClock, CMTime, CMTimeRange, CMTimebase};
use std::thread;
use std::time::Duration;

#[test]
fn cm_time_zero_is_numeric_zero() {
    assert_eq!(
        CMTime::ZERO,
        CMTime {
            value: 0,
            timescale: 1,
            flags: 1,
            epoch: 0,
        }
    );
    assert_eq!(CMTime::ZERO.as_seconds(), Some(0.0));
    assert!(CMTime::ZERO.is_numeric());
    assert!(CMTime::ZERO.is_zero());
    assert!(CMTime {
        value: 0,
        timescale: 0,
        flags: 1,
        epoch: 0,
    }
    .is_zero());
}

#[test]
fn cm_time_rounded_values_are_not_indefinite() {
    let rounded = CMTime::new(1, 3).convert_scale(10);
    assert!(rounded.is_numeric());
    assert!(rounded.has_been_rounded());
    assert!(!rounded.is_indefinite());
}

#[test]
fn cm_time_default_scale_conversion_rounds_half_away_from_zero() {
    let positive = CMTime::new(1, 2).convert_scale(1);
    let negative = CMTime::new(-1, 2).convert_scale(1);

    assert_eq!(positive.value, 1);
    assert_eq!(negative.value, -1);
    assert!(positive.has_been_rounded());
    assert!(negative.has_been_rounded());
}

#[test]
fn cm_time_indefinite_arithmetic_stays_indefinite() {
    let indefinite = CMTime::indefinite();
    assert_eq!(
        indefinite,
        CMTime {
            value: 0,
            timescale: 0,
            flags: 0x11,
            epoch: 0,
        }
    );
    assert!(!indefinite.is_numeric());
    assert!(!indefinite.is_zero());
    assert_eq!(indefinite.as_seconds(), None);

    let one_second = CMTime::new(1, 1);
    for result in [
        indefinite.add(one_second),
        indefinite.subtract(one_second),
        indefinite.multiply(2),
        indefinite.multiply_by_f64(2.0),
        indefinite.convert_scale(600),
    ] {
        assert!(result.is_indefinite(), "result was {result:?}");
        assert!(!result.is_numeric());
    }
}

#[test]
fn cm_time_rejects_nonpositive_timescales() {
    assert_eq!(CMTime::new(1, 0), CMTime::INVALID);
    assert_eq!(CMTime::new(1, -1), CMTime::INVALID);
}

#[test]
fn cm_time_range_helpers_work() {
    let range = CMTimeRange::new(CMTime::new(0, 600), CMTime::new(300, 600));
    assert_eq!(range.end(), CMTime::new(300, 600));
    assert!(range.contains_time(CMTime::new(150, 600)));
    assert!(!range.contains_time(CMTime::new(400, 600)));
}

#[test]
fn cm_timebase_smoke() {
    let clock = CMClock::host_time_clock();
    let timebase = CMTimebase::with_source_clock(&clock).expect("timebase");
    assert_eq!(timebase.set_rate(1.0), 0);
    assert_eq!(timebase.set_time(CMTime::new(0, 600)), 0);
    assert!(timebase.time().is_valid());
    assert!(timebase.source_clock().is_some());
}

#[test]
fn host_time_clock_reports_an_advancing_valid_time() {
    let clock = CMClock::host_time_clock();
    let first = clock.time();
    assert!(first.is_valid());
    thread::sleep(Duration::from_millis(5));
    let second = clock.time();
    assert!(second.is_valid());
    assert!(second.as_seconds() > first.as_seconds());
    let clone = clock.clone();
    drop(clock);
    assert!(clone.time().is_valid());
}
