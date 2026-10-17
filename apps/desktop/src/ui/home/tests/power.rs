use super::*;

#[gpui::test]
fn idle_sleep_toggle_persists_and_tracks_daemon_status(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let directory = tempfile::tempdir().unwrap();
    let paths = removent_core::DataPaths {
        root: directory.path().into(),
    };
    let (commands, _rx) = tokio::sync::mpsc::channel(1);
    let engine = Engine::for_ui_test(paths.clone(), commands);
    let slot = Rc::new(std::cell::RefCell::new(None));
    let out = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let view = cx.new(|cx| HomeView::new(engine, window, cx));
        view.update(cx, |view, _| {
            view.settings_open = true;
            view.settings_section = 4;
        });
        *out.borrow_mut() = Some(view.clone());
        gpui_component::Root::new(AnyView::from(view), window, cx)
    });
    let view = slot.borrow_mut().take().unwrap();
    cx.simulate_resize(gpui::size(px(860.), px(700.)));
    for (mode, enabled) in [
        (gpui_component::ThemeMode::Dark, true),
        (gpui_component::ThemeMode::Light, false),
    ] {
        cx.update(|window, cx| {
            gpui_component::Theme::change(mode, Some(window), cx);
        });
        cx.run_until_parked();
        let bounds = cx.debug_bounds("prevent-idle-sleep-toggle").unwrap();
        assert!(bounds.right() <= px(860.) && bounds.top() >= px(0.));
        cx.simulate_click(bounds.center(), Default::default());
        cx.run_until_parked();
        assert_eq!(
            removent_core::Settings::load(&paths)
                .unwrap()
                .prevent_idle_sleep,
            enabled
        );
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.handle_event(
                    UiEvent::DaemonPower {
                        enabled,
                        active: enabled,
                        error: None,
                    },
                    window,
                    cx,
                );
                assert_eq!(view.daemon_power, Some((enabled, enabled, None)));
            })
        });
    }
}
