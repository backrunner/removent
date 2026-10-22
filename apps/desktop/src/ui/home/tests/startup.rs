use super::*;
use crate::engine::StartupSettings;
use removent_core::system_service::SystemServiceStatus;

#[gpui::test]
fn startup_setup_blocks_duplicate_actions_and_clears_resolved_errors(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let directory = tempfile::tempdir().unwrap();
    let paths = removent_core::DataPaths {
        root: directory.path().into(),
    };
    let (commands, _rx) = tokio::sync::mpsc::channel(1);
    let engine = Engine::for_ui_test(paths.clone(), commands);
    engine.set_startup_settings_for_test(StartupSettings {
        busy: true,
        launch_at_login: Some(false),
        system: Some(SystemServiceStatus::default()),
        ..Default::default()
    });
    let control = engine.clone();
    let slot = Rc::new(std::cell::RefCell::new(None));
    let out = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let view = cx.new(|cx| HomeView::new(engine, window, cx));
        view.update(cx, |view, _| {
            view.settings_open = true;
            view.settings_section = 0;
        });
        *out.borrow_mut() = Some(view.clone());
        gpui_component::Root::new(AnyView::from(view), window, cx)
    });
    let view = slot.borrow_mut().take().unwrap();
    cx.simulate_resize(gpui::size(px(860.), px(600.)));
    for mode in [
        gpui_component::ThemeMode::Dark,
        gpui_component::ThemeMode::Light,
    ] {
        cx.update(|window, cx| gpui_component::Theme::change(mode, Some(window), cx));
        cx.run_until_parked();
        for selector in ["install-system-startup", "launch-at-login-toggle"] {
            let bounds = cx.debug_bounds(selector).unwrap();
            assert!(bounds.right() <= px(860.) && bounds.top() >= px(0.));
            cx.simulate_click(bounds.center(), Default::default());
        }
        cx.run_until_parked();
        assert!(control.startup_settings().busy);
        assert!(!paths.run_dir().join("system-startup-configured").exists());
        assert!(!paths.run_dir().join("login-startup-configured").exists());
    }
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            control.set_startup_settings_for_test(StartupSettings {
                error: Some("Synthetic startup failure".into()),
                ..Default::default()
            });
            view.handle_event(UiEvent::StartupChanged, window, cx);
            assert_eq!(view.status, "Synthetic startup failure");
            control.set_startup_settings_for_test(StartupSettings {
                launch_at_login: Some(true),
                system: Some(SystemServiceStatus::default()),
                ..Default::default()
            });
            view.handle_event(UiEvent::StartupChanged, window, cx);
            assert_eq!(view.status, t!("status.ready").to_string());
        })
    });
}
