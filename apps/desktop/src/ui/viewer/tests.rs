use super::*;
use gpui::{AnyView, Entity, KeyBinding, TestAppContext, VisualTestContext, point};
use removent_proto::ControlMsg;

struct Fixture {
    viewer: Entity<ViewerView>,
    commands: tokio::sync::mpsc::Receiver<ControlMsg>,
    _frames: removent_core::latest::Sender<VideoFrame>,
    _directory: tempfile::TempDir,
}

fn setup(cx: &mut TestAppContext) -> (Fixture, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.bind_keys([KeyBinding::new(
            "ctrl-cmd-i",
            ViewerToggleInfo,
            Some("Viewer"),
        )]);
    });
    let directory = tempfile::tempdir().unwrap();
    let (commands, mut commands_rx) = tokio::sync::mpsc::channel(64);
    let engine = Engine::for_viewer_test(
        removent_core::DataPaths {
            root: directory.path().into(),
        },
        commands,
    );
    let (frames, frames_rx) = removent_core::latest::channel();
    let viewer_slot = std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = viewer_slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let viewer =
            cx.new(|cx| ViewerView::new(engine, frames_rx, "Remote Mac".into(), window, cx));
        viewer.update(cx, |view, _| {
            view.handle_frame(
                VideoFrame {
                    width: 640,
                    height: 360,
                    data: vec![0; 640 * 360 * 4],
                    pts_us: 0,
                },
                window,
            );
        });
        *slot.borrow_mut() = Some(viewer.clone());
        gpui_component::Root::new(AnyView::from(viewer), window, cx)
    });
    let viewer = viewer_slot.borrow_mut().take().unwrap();
    while commands_rx.try_recv().is_ok() {}
    (
        Fixture {
            viewer,
            commands: commands_rx,
            _frames: frames,
            _directory: directory,
        },
        cx,
    )
}

#[gpui::test]
fn info_toggle_and_panel_clicks_stay_local(cx: &mut TestAppContext) {
    let (mut fixture, cx) = setup(cx);
    cx.run_until_parked();
    assert!(cx.debug_bounds("viewer-info").is_none());
    cx.simulate_keystrokes("ctrl-cmd-i");
    cx.run_until_parked();
    let panel = cx.debug_bounds("viewer-info").unwrap();
    assert_eq!(panel.left(), px(16.));
    assert_eq!(panel.top(), TITLE_BAR_HEIGHT + px(16.));
    for button in [MouseButton::Left, MouseButton::Right, MouseButton::Middle] {
        cx.simulate_mouse_down(panel.center(), button, Default::default());
        cx.simulate_mouse_up(panel.center(), button, Default::default());
    }
    assert!(
        fixture.commands.try_recv().is_err(),
        "panel interactions must not move or click remotely"
    );
    let close = cx.debug_bounds("close-viewer-info").unwrap();
    cx.simulate_mouse_move(close.center(), None, Default::default());
    cx.simulate_click(close.center(), Default::default());
    cx.run_until_parked();
    cx.update(|_, cx| {
        fixture.viewer.update(cx, |view, _| {
            assert!(!view.info_visible);
            view.forward_key(&Keystroke::parse("i").unwrap(), KeyKind::Up);
        })
    });
    assert!(
        fixture.commands.try_recv().is_err(),
        "releasing a local shortcut must not send a stray key-up"
    );
    cx.update(|_, cx| {
        fixture.viewer.update(cx, |view, _| {
            view.forward_key(&Keystroke::parse("i").unwrap(), KeyKind::Down);
            view.forward_key(&Keystroke::parse("ctrl-cmd-i").unwrap(), KeyKind::Up);
        })
    });
    assert!(matches!(
        fixture.commands.try_recv().unwrap(),
        ControlMsg::KeyEvent {
            kind: KeyKind::Down,
            ..
        }
    ));
    assert!(
        matches!(
            fixture.commands.try_recv().unwrap(),
            ControlMsg::KeyEvent {
                kind: KeyKind::Up,
                ..
            }
        ),
        "a remotely held key must release even if local shortcut modifiers were added"
    );
}

#[gpui::test]
fn releasing_a_remote_drag_over_info_does_not_leave_a_held_button(cx: &mut TestAppContext) {
    let (mut fixture, cx) = setup(cx);
    cx.simulate_resize(size(px(1000.), px(700.)));
    cx.simulate_keystrokes("ctrl-cmd-i");
    cx.run_until_parked();
    let panel = cx.debug_bounds("viewer-info").unwrap();
    for (button, mask, down, up) in [
        (MouseButton::Left, 1, MouseKind::LeftDown, MouseKind::LeftUp),
        (
            MouseButton::Right,
            2,
            MouseKind::RightDown,
            MouseKind::RightUp,
        ),
        (
            MouseButton::Middle,
            4,
            MouseKind::MiddleDown,
            MouseKind::MiddleUp,
        ),
    ] {
        cx.simulate_mouse_down(point(px(800.), px(350.)), button, Default::default());
        cx.simulate_mouse_move(panel.center(), button, Default::default());
        cx.simulate_mouse_up(panel.center(), button, Default::default());
        let events: Vec<_> = std::iter::from_fn(|| fixture.commands.try_recv().ok()).collect();
        assert!(matches!(
            events.first(),
            Some(ControlMsg::MouseEvent { kind, buttons, .. })
                if *kind == down && *buttons == mask
        ));
        assert!(matches!(
            events.last(),
            Some(ControlMsg::MouseEvent { kind, buttons: 0, .. }) if *kind == up
        ));
        cx.update(|_, cx| {
            fixture
                .viewer
                .update(cx, |view, _| assert_eq!(view.buttons, 0))
        });
    }
}

#[gpui::test]
fn full_info_fits_minimum_window_and_idle_samples_reset_fps(cx: &mut TestAppContext) {
    let (mut fixture, cx) = setup(cx);
    cx.simulate_resize(size(px(480.), px(320.)));
    cx.simulate_keystrokes("ctrl-cmd-i");
    cx.update(|_, cx| {
        fixture.viewer.update(cx, |view, cx| {
            view.toolbar_until = Some(Instant::now() + TOOLBAR_HIDE_AFTER);
            view.diagnostics.input = Some(Default::default());
            view.diagnostics.vnc = Some(Default::default());
            cx.notify();
        })
    });
    cx.run_until_parked();
    let panel = cx.debug_bounds("viewer-info").unwrap();
    let close = cx.debug_bounds("close-viewer-info").unwrap();
    assert!(panel.bottom() <= px(320. - 16.), "{panel:?}");
    assert!(panel.right() <= px(480.));
    assert!(close.bottom() < panel.bottom());
    let toolbar = cx.debug_bounds("viewer-toolbar-chip").unwrap();
    assert!(
        toolbar.left() > panel.right(),
        "{toolbar:?} overlaps {panel:?}"
    );
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: panel.center(),
        delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-100.))),
        touch_phase: TouchPhase::Moved,
        modifiers: Default::default(),
    });
    assert!(
        fixture.commands.try_recv().is_err(),
        "scrolling performance info must stay local"
    );
    cx.update(|_, cx| {
        fixture.viewer.update(cx, |view, _| {
            assert!(
                view.info_scroll.offset().y < px(0.),
                "all diagnostics must be reachable by scrolling"
            );
            view.fps_counter = 30;
            view.last_fps_tick = Instant::now() - Duration::from_secs(2);
            view.sample_diagnostics();
            assert!((14.9..=15.1).contains(&view.fps_shown));
            view.last_fps_tick = Instant::now() - Duration::from_secs(1);
            view.sample_diagnostics();
            assert_eq!(view.fps_shown, 0.);
        })
    });
}

#[test]
fn keystrokes_preserve_caps_lock_from_modifier_notifications() {
    let modifiers = gpui::Modifiers {
        shift: true,
        ..Default::default()
    };
    assert_eq!(
        key_modifiers(&modifiers, KeyModifiers::CAPS_LOCK),
        KeyModifiers::SHIFT | KeyModifiers::CAPS_LOCK
    );
    assert_eq!(
        key_modifiers(&modifiers, KeyModifiers::empty()),
        KeyModifiers::SHIFT
    );
}

#[test]
fn escape_and_find_belong_to_remote_except_explicit_local_chords() {
    for (key, local) in [
        ("escape", false),
        ("cmd-f", false),
        ("ctrl-cmd-escape", true),
        ("ctrl-cmd-f", true),
        ("ctrl-cmd-i", true),
        ("cmd-i", false),
    ] {
        assert_eq!(
            is_viewer_shortcut(&Keystroke::parse(key).unwrap()),
            local,
            "{key}"
        );
    }
}
