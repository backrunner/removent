use super::*;
use gpui::{AnyView, TestAppContext};

#[gpui::test]
fn cloud_sync_switch_persists_without_requiring_cloud_access(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let directory = tempfile::tempdir().unwrap();
    let paths = removent_core::DataPaths {
        root: directory.path().into(),
    };
    let (commands, _rx) = tokio::sync::mpsc::channel(1);
    let engine = Engine::for_viewer_test(paths.clone(), commands);
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let view = cx.new(|cx| HomeView::new(engine, window, cx));
        view.update(cx, |view, _| {
            view.settings_open = true;
            view.settings_section = 6;
        });
        gpui_component::Root::new(AnyView::from(view), window, cx)
    });
    cx.simulate_resize(gpui::size(px(860.), px(650.)));
    cx.run_until_parked();
    for expected in [true, false] {
        let bounds = cx
            .debug_bounds("cloud-sync-toggle")
            .expect("iCloud switch renders");
        assert!(bounds.right() <= px(860.) && bounds.bottom() <= px(650.));
        cx.simulate_mouse_move(bounds.center(), None, Default::default());
        cx.simulate_click(bounds.center(), Default::default());
        cx.run_until_parked();
        let status = removent_client::cloud_sync::dispatch(
            &paths,
            removent_client::cloud_sync::Command::Status,
        )
        .unwrap();
        assert_eq!(status["enabled"], expected);
        if expected {
            assert_eq!(status["code"], "configuration");
        }
    }
}

#[gpui::test]
fn update_channel_controls_persist_and_ignore_obsolete_ready_events(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let directory = tempfile::tempdir().unwrap();
    let paths = removent_core::DataPaths {
        root: directory.path().into(),
    };
    let (commands, _rx) = tokio::sync::mpsc::channel(1);
    let engine = Engine::for_viewer_test(paths.clone(), commands);
    // curl rejects non-HTTPS before connecting; UI tests never hit GitHub.
    engine
        .update_settings(|s| s.update_endpoint = "http://127.0.0.1:1/latest.json".into())
        .unwrap();
    let slot = Rc::new(std::cell::RefCell::new(None));
    let out = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let view = cx.new(|cx| HomeView::new(engine, window, cx));
        view.update(cx, |view, cx| {
            view.settings_open = true;
            view.settings_section = 4;
            view.handle_event(
                UiEvent::UpdateStatus(UpdateStatus::ReadyToInstall {
                    version: "9.0.0-beta.1".into(),
                }),
                window,
                cx,
            );
            assert_eq!(view.update_status, UpdateStatus::Idle);
        });
        *out.borrow_mut() = Some(view.clone());
        gpui_component::Root::new(AnyView::from(view), window, cx)
    });
    let view = slot.borrow_mut().take().unwrap();
    cx.simulate_resize(gpui::size(px(860.), px(1100.)));
    cx.run_until_parked();
    let stable = cx
        .debug_bounds("update-channel-0")
        .expect("stable choice renders");
    let beta = cx
        .debug_bounds("update-channel-1")
        .expect("beta choice renders");
    assert!(stable.left() >= px(0.) && beta.right() <= px(860.));
    assert!(beta.bottom() <= px(1100.));
    cx.simulate_mouse_move(beta.center(), None, Default::default());
    cx.simulate_click(beta.center(), Default::default());
    cx.run_until_parked();
    assert_eq!(
        removent_core::Settings::load(&paths)
            .unwrap()
            .update_channel,
        UpdateChannel::Beta
    );
    cx.update(|_, cx| {
        assert_eq!(
            view.read(cx).engine.settings().update_channel,
            UpdateChannel::Beta
        )
    });
}

#[gpui::test]
fn discovery_switches_persist_independently_and_remove_disabled_rows(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let directory = tempfile::tempdir().unwrap();
    let paths = removent_core::DataPaths {
        root: directory.path().into(),
    };
    let (commands, _rx) = tokio::sync::mpsc::channel(1);
    let engine = Engine::for_viewer_test(paths.clone(), commands);
    let slot = Rc::new(std::cell::RefCell::new(None));
    let out = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let view = cx.new(|cx| HomeView::new(engine, window, cx));
        view.update(cx, |view, _| {
            view.settings_open = true;
            view.settings_section = 5;
        });
        *out.borrow_mut() = Some(view.clone());
        gpui_component::Root::new(AnyView::from(view), window, cx)
    });
    let view = slot.borrow_mut().take().unwrap();
    cx.simulate_resize(gpui::size(px(860.), px(600.)));
    cx.run_until_parked();
    for (selector, expected) in [
        (
            "discovery-enabled-1",
            removent_core::DiscoverySettings {
                removent: true,
                vnc: true,
                rdp: false,
            },
        ),
        (
            "discovery-enabled-2",
            removent_core::DiscoverySettings {
                removent: true,
                vnc: true,
                rdp: true,
            },
        ),
        (
            "discovery-enabled-0",
            removent_core::DiscoverySettings {
                removent: false,
                vnc: true,
                rdp: true,
            },
        ),
    ] {
        let bounds = cx
            .debug_bounds(selector)
            .expect("discovery switch must render");
        assert!(bounds.bottom() <= px(600.));
        cx.simulate_mouse_move(bounds.center(), None, Default::default());
        cx.simulate_click(bounds.center(), Default::default());
        cx.run_until_parked();
        assert_eq!(
            removent_core::Settings::load(&paths).unwrap().discovery,
            expected
        );
    }
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.handle_event(
                UiEvent::DeviceFound {
                    fp: "VNC:192.168.1.2:5900".into(),
                    name: "Office".into(),
                    addr: "192.168.1.2:5900".parse().unwrap(),
                    protocol: ConnectionProtocol::Vnc,
                },
                window,
                cx,
            );
            view.selected = Some(Selection::Device("VNC:192.168.1.2:5900".into()));
        })
    });
    cx.run_until_parked();
    let bounds = cx.debug_bounds("discovery-enabled-1").unwrap();
    cx.simulate_mouse_move(bounds.center(), None, Default::default());
    cx.simulate_click(bounds.center(), Default::default());
    cx.run_until_parked();
    cx.update(|_, cx| {
        let view = view.read(cx);
        assert!(view.devices.is_empty());
        assert!(view.selected.is_none());
    });
    let settings = removent_core::Settings::load(&paths).unwrap();
    assert!(!settings.discovery.vnc);
    assert!(settings.discovery.rdp);
    assert!(
        !settings.vnc_enabled,
        "discovery must not enable the local VNC server"
    );
}
