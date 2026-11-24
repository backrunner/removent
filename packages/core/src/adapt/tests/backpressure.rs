use super::*;

#[test]
fn sustained_write_backpressure_drops_early_without_shortening_recovery() {
    let mut controller = AdaptationController::with_video_quality(
        12_000,
        30,
        removent_proto::VideoQuality::Standard,
        1920,
        1080,
    );
    for _ in 0..2 {
        assert!(
            controller
                .on_sample_with_write_backpressure(&good(), 250, true)
                .is_none()
        );
    }
    assert_eq!(
        controller
            .on_sample_with_write_backpressure(&good(), 250, true)
            .unwrap()
            .bitrate_kbps,
        8400
    );
    for _ in 0..4 {
        assert!(
            controller
                .on_sample_with_write_backpressure(&good(), 250, true)
                .is_none()
        );
    }
    for _ in 0..20 {
        assert!(controller.on_sample(&good(), 250).is_none());
    }
    assert_eq!(
        controller.on_sample(&good(), 250).unwrap().bitrate_kbps,
        10_500
    );
    for _ in 0..7 {
        controller.on_sample(&good(), 250);
    }
    for _ in 0..2 {
        assert!(
            controller
                .on_sample_with_write_backpressure(&good(), 250, true)
                .is_none()
        );
    }
    assert!(
        controller
            .on_sample_with_write_backpressure(&good(), 250, true)
            .is_some()
    );
}

#[test]
fn isolated_write_backpressure_does_not_fast_cut_on_persistent_loss() {
    let mut controller = AdaptationController::new(12_000, 30, QualityPreset::Auto);
    assert!(
        controller
            .on_sample_with_write_backpressure(&bad(), 250, true)
            .is_none()
    );
    for _ in 0..6 {
        assert!(controller.on_sample(&bad(), 250).is_none());
    }
    assert!(controller.on_sample(&bad(), 250).is_some());
}

#[test]
fn fast_first_cut_keeps_the_next_cut_on_the_normal_schedule() {
    let mut controller = AdaptationController::new(12_000, 30, QualityPreset::Auto);
    for tick in 1..=16 {
        let changed = controller
            .on_sample_with_write_backpressure(&good(), 250, true)
            .is_some();
        assert_eq!(changed, matches!(tick, 3 | 16), "at tick {tick}");
    }
}
