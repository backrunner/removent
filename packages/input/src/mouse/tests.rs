use super::*;

fn build(
    injector: &mut MouseInjector,
    kind: MouseKind,
    x: f64,
    ms: u64,
    start: Instant,
) -> CGEvent {
    injector
        .build(
            kind,
            x,
            20.,
            CGEventFlags::CGEventFlagCommand,
            start + Duration::from_millis(ms),
            Duration::from_millis(500),
        )
        .unwrap()
}

fn count(event: &CGEvent) -> i64 {
    event.get_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE)
}

#[test]
fn single_double_triple_clicks_keep_counts_and_matching_up_numbers_for_every_button() {
    for (down, up, drag, ty) in [
        (
            MouseKind::LeftDown,
            MouseKind::LeftUp,
            MouseKind::LeftDragged,
            CGEventType::LeftMouseDragged,
        ),
        (
            MouseKind::RightDown,
            MouseKind::RightUp,
            MouseKind::RightDragged,
            CGEventType::RightMouseDragged,
        ),
        (
            MouseKind::MiddleDown,
            MouseKind::MiddleUp,
            MouseKind::MiddleDragged,
            CGEventType::OtherMouseDragged,
        ),
    ] {
        let mut injector = MouseInjector::default();
        let start = Instant::now();
        let mut previous_number = 0;
        for click in 1..=3 {
            let ms = click as u64 * 100;
            let down = build(&mut injector, down, 10., ms, start);
            let dragged = build(&mut injector, drag, 11., ms + 10, start);
            let up = build(&mut injector, up, 11., ms + 20, start);
            assert_eq!(dragged.get_type() as u32, ty as u32);
            let number = down.get_integer_value_field(EventField::MOUSE_EVENT_NUMBER);
            assert!(number > previous_number);
            previous_number = number;
            for event in [down, dragged, up] {
                assert!(crate::origin::is_our_event(&event));
                assert_eq!(count(&event), click);
                assert_eq!(
                    event.get_integer_value_field(EventField::MOUSE_EVENT_NUMBER),
                    number
                );
                assert!(event.get_flags().contains(CGEventFlags::CGEventFlagCommand));
            }
        }
    }
}

#[test]
fn slow_distant_and_interrupted_clicks_start_a_new_sequence() {
    let start = Instant::now();
    for (kind, x, ms) in [
        (MouseKind::LeftDown, 10., 700),
        (MouseKind::LeftDown, 30., 100),
        (MouseKind::RightDown, 10., 100),
    ] {
        let mut injector = MouseInjector::default();
        build(&mut injector, MouseKind::LeftDown, 10., 0, start);
        build(&mut injector, MouseKind::LeftUp, 10., 10, start);
        assert_eq!(count(&build(&mut injector, kind, x, ms, start)), 1);
    }
    let mut injector = MouseInjector::default();
    build(&mut injector, MouseKind::LeftDown, 10., 0, start);
    build(&mut injector, MouseKind::LeftDragged, 40., 10, start);
    // Returning to the original point after a drag must not become a double click.
    build(&mut injector, MouseKind::LeftUp, 10., 20, start);
    assert_eq!(
        count(&build(&mut injector, MouseKind::LeftDown, 10., 30, start)),
        1
    );
}

#[test]
fn simultaneous_buttons_keep_independent_matching_releases() {
    let mut injector = MouseInjector::default();
    let start = Instant::now();
    let left = build(&mut injector, MouseKind::LeftDown, 10., 0, start);
    let right = build(&mut injector, MouseKind::RightDown, 10., 10, start);
    for (down, kind) in [(left, MouseKind::LeftUp), (right, MouseKind::RightUp)] {
        let up = build(&mut injector, kind, 10., 20, start);
        assert_eq!(
            up.get_integer_value_field(EventField::MOUSE_EVENT_NUMBER),
            down.get_integer_value_field(EventField::MOUSE_EVENT_NUMBER)
        );
    }
    assert_eq!(
        count(&build(&mut injector, MouseKind::LeftDown, 10., 30, start)),
        1
    );
}

#[test]
fn sessions_do_not_share_click_history() {
    let start = Instant::now();
    let mut first = MouseInjector::default();
    build(&mut first, MouseKind::LeftDown, 10., 0, start);
    build(&mut first, MouseKind::LeftUp, 10., 10, start);
    assert_eq!(
        count(&build(
            &mut MouseInjector::default(),
            MouseKind::LeftDown,
            10.,
            20,
            start
        )),
        1
    );
}
