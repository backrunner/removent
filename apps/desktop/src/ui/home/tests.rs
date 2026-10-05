use super::*;
use gpui::{AnyView, TestAppContext};

#[gpui::test]
fn settings_sidebar_switches_sections_and_preserves_device_selection(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let directory = tempfile::tempdir().unwrap();
    let paths = removent_core::DataPaths {
        root: directory.path().into(),
    };
    let (commands, _rx) = tokio::sync::mpsc::channel(1);
    let engine = Engine::for_viewer_test(paths, commands);
    let slot = Rc::new(std::cell::RefCell::new(None));
    let out = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let view = cx.new(|cx| HomeView::new(engine, window, cx));
        view.update(cx, |view, _| {
            view.devices.insert(
                "office".into(),
                DeviceRow {
                    name: "Office Mac".into(),
                    addr: "192.0.2.1:48688".parse().unwrap(),
                    protocol: ConnectionProtocol::Removent,
                },
            );
            view.selected = Some(Selection::Device("office".into()));
            view.settings_open = true;
        });
        *out.borrow_mut() = Some(view.clone());
        gpui_component::Root::new(AnyView::from(view), window, cx)
    });
    let view = slot.borrow_mut().take().unwrap();
    cx.simulate_resize(gpui::size(px(860.), px(600.)));
    cx.run_until_parked();
    for (section, selector) in [
        (1, "settings-nav-1"),
        (2, "settings-nav-2"),
        (3, "settings-nav-3"),
        (5, "settings-nav-5"),
        (6, "settings-nav-6"),
        (4, "settings-nav-4"),
        (0, "settings-nav-0"),
    ] {
        let bounds = cx
            .debug_bounds(selector)
            .expect("settings category is available without scrolling");
        assert!(bounds.left() >= px(0.) && bounds.right() <= px(228.));
        assert!(bounds.bottom() <= px(600.));
        cx.simulate_mouse_move(bounds.center(), None, Default::default());
        cx.simulate_click(bounds.center(), Default::default());
        cx.run_until_parked();
        cx.update(|_, cx| {
            let state = view.read(cx);
            assert_eq!(state.settings_section, section);
            assert!(state.settings_open);
            assert!(matches!(&state.selected, Some(Selection::Device(fp)) if fp == "office"));
        });
        if section == 6 {
            assert!(cx.debug_bounds("cloud-sync-toggle").is_some());
        }
    }
    let back = cx.debug_bounds("settings-back").unwrap();
    cx.simulate_mouse_move(back.center(), None, Default::default());
    cx.simulate_click(back.center(), Default::default());
    cx.run_until_parked();
    cx.update(|_, cx| {
        let state = view.read(cx);
        assert!(!state.settings_open);
        assert!(matches!(&state.selected, Some(Selection::Device(fp)) if fp == "office"));
    });
}

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

#[gpui::test]
fn authentication_settings_persist_each_mode_and_require_password(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let directory = tempfile::tempdir().unwrap();
    let paths = removent_core::DataPaths {
        root: directory.path().into(),
    };
    let read_paths = paths.clone();
    let (commands, _rx) = tokio::sync::mpsc::channel(1);
    let engine = Engine::for_viewer_test(paths, commands);
    let slot = Rc::new(std::cell::RefCell::new(None));
    let out = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let view = cx.new(|cx| HomeView::new(engine, window, cx));
        view.update(cx, |view, _| {
            view.settings_open = true;
            view.settings_section = 2;
        });
        *out.borrow_mut() = Some(view.clone());
        gpui_component::Root::new(AnyView::from(view), window, cx)
    });
    let view = slot.borrow_mut().take().unwrap();
    cx.simulate_resize(gpui::size(px(860.), px(700.)));
    cx.run_until_parked();
    let password = cx.debug_bounds("authentication-1").unwrap();
    cx.simulate_mouse_move(password.center(), None, Default::default());
    cx.simulate_click(password.center(), Default::default());
    cx.run_until_parked();
    assert_eq!(
        removent_core::Settings::load(&read_paths)
            .unwrap()
            .authentication
            .mode,
        removent_core::AuthenticationMode::PairingCode
    );
    view.update_in(cx, |view, window, cx| {
        view.auth_password_input.update(cx, |input, cx| {
            input.set_value("synthetic-host-password", window, cx)
        });
        view.save_authentication(removent_core::AuthenticationMode::Password, cx);
    });
    assert_eq!(
        removent_core::Settings::load(&read_paths)
            .unwrap()
            .authentication
            .mode,
        removent_core::AuthenticationMode::Password
    );
    for (selector, mode) in [
        ("authentication-2", removent_core::AuthenticationMode::Otp),
        ("authentication-3", removent_core::AuthenticationMode::None),
        (
            "authentication-0",
            removent_core::AuthenticationMode::PairingCode,
        ),
    ] {
        cx.run_until_parked();
        let bounds = cx.debug_bounds(selector).unwrap();
        assert!(bounds.right() <= px(860.));
        cx.simulate_mouse_move(bounds.center(), None, Default::default());
        cx.simulate_click(bounds.center(), Default::default());
        cx.run_until_parked();
        let settings = removent_core::Settings::load(&read_paths).unwrap();
        assert_eq!(settings.authentication.mode, mode);
        if mode == removent_core::AuthenticationMode::Otp {
            assert!(settings.authentication.totp().is_ok());
        }
    }
}

#[gpui::test]
fn certificate_confirmation_requires_a_click_and_rejects_stale_events(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let directory = tempfile::tempdir().unwrap();
    let paths = removent_core::DataPaths {
        root: directory.path().into(),
    };
    let (commands, _rx) = tokio::sync::mpsc::channel(1);
    let engine = Engine::for_viewer_test(paths, commands);
    let slot = Rc::new(std::cell::RefCell::new(None));
    let out = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let view = cx.new(|cx| HomeView::new(engine, window, cx));
        *out.borrow_mut() = Some(view.clone());
        gpui_component::Root::new(AnyView::from(view), window, cx)
    });
    let view = slot.borrow_mut().take().unwrap();
    cx.simulate_resize(gpui::size(px(860.), px(650.)));
    let (tx, mut stale) = oneshot::channel();
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.handle_event(
                UiEvent::ConfirmCertificate {
                    generation: view.engine.client_generation() + 1,
                    destination: "stale".into(),
                    relay: false,
                    tx,
                },
                window,
                cx,
            );
            assert!(view.pin_dialog.is_none());
        })
    });
    assert!(matches!(
        stale.try_recv(),
        Err(oneshot::error::TryRecvError::Closed)
    ));
    let (tx, mut answer) = oneshot::channel();
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.handle_event(
                UiEvent::ConfirmCertificate {
                    generation: view.engine.client_generation(),
                    destination: "Office Mac".into(),
                    relay: false,
                    tx,
                },
                window,
                cx,
            );
        })
    });
    cx.run_until_parked();
    assert!(matches!(
        answer.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    let button = cx.debug_bounds("trust-certificate").unwrap();
    cx.simulate_click(button.center(), Default::default());
    cx.run_until_parked();
    assert!(answer.try_recv().unwrap());
    let (tx, mut cancel) = oneshot::channel();
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.handle_event(
                UiEvent::ConfirmCertificate {
                    generation: view.engine.client_generation(),
                    destination: "relay.example".into(),
                    relay: true,
                    tx,
                },
                window,
                cx,
            );
        })
    });
    cx.run_until_parked();
    let button = cx.debug_bounds("cancel-certificate").unwrap();
    cx.simulate_click(button.center(), Default::default());
    cx.run_until_parked();
    assert!(!cancel.try_recv().unwrap());
}
