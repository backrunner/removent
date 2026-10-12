use super::*;
use gpui::{AnyView, TestAppContext};

mod authentication;

#[gpui::test]
fn permissions_clear_during_restart_and_reappear_from_the_new_snapshot(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let directory = tempfile::tempdir().unwrap();
    let paths = removent_core::DataPaths {
        root: directory.path().into(),
    };
    let (commands, _rx) = tokio::sync::mpsc::channel(1);
    let engine = Engine::for_ui_test(paths, commands);
    let slot = Rc::new(std::cell::RefCell::new(None));
    let out = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let view = cx.new(|cx| HomeView::new(engine, window, cx));
        *out.borrow_mut() = Some(view.clone());
        gpui_component::Root::new(AnyView::from(view), window, cx)
    });
    let view = slot.borrow_mut().take().unwrap();
    cx.simulate_resize(gpui::size(px(1040.), px(1000.)));
    for event in [
        UiEvent::DaemonOnline(true),
        UiEvent::DaemonPermissions {
            screen_recording: true,
            accessibility: true,
        },
        UiEvent::DaemonOnline(false),
        UiEvent::DaemonOnline(true),
        UiEvent::DaemonPermissions {
            screen_recording: true,
            accessibility: true,
        },
        UiEvent::DaemonPermissions {
            screen_recording: true,
            accessibility: false,
        },
    ] {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| view.handle_event(event, window, cx));
        });
        cx.run_until_parked();
        let grants = cx.update(|_, cx| view.read(cx).daemon_perms);
        for (selectors, grant) in [
            (
                [
                    "permission-granted-screen-capture",
                    "permission-settings-screen-capture",
                    "permission-unknown-screen-capture",
                ],
                grants.map(|pair| pair.0),
            ),
            (
                [
                    "permission-granted-accessibility",
                    "permission-settings-accessibility",
                    "permission-unknown-accessibility",
                ],
                grants.map(|pair| pair.1),
            ),
        ] {
            for (selector, visible) in selectors.into_iter().zip([
                grant == Some(true),
                grant == Some(false),
                grant.is_none(),
            ]) {
                assert_eq!(cx.debug_bounds(selector).is_some(), visible);
            }
        }
    }
}

#[gpui::test]
fn quick_discovery_enable_persists_and_keeps_retry_available_on_save_failure(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_component::init);
    let directory = tempfile::tempdir().unwrap();
    let paths = removent_core::DataPaths {
        root: directory.path().into(),
    };
    let (commands, _rx) = tokio::sync::mpsc::channel(1);
    let engine = Engine::for_ui_test(paths.clone(), commands);
    engine
        .update_settings(|s| {
            s.discovery.removent = false;
            s.discovery.vnc = false;
            s.discovery.rdp = false;
            s.host_enabled = false;
        })
        .unwrap();
    let before = engine.settings();
    let slot = Rc::new(std::cell::RefCell::new(None));
    let out = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let view = cx.new(|cx| HomeView::new(engine, window, cx));
        *out.borrow_mut() = Some(view.clone());
        gpui_component::Root::new(AnyView::from(view), window, cx)
    });
    let view = slot.borrow_mut().take().unwrap();
    cx.simulate_resize(gpui::size(px(860.), px(600.)));
    cx.run_until_parked();

    // A failed disk write must leave the disabled state and its retry action intact.
    std::fs::remove_file(paths.settings_file()).unwrap();
    std::fs::create_dir(paths.settings_file()).unwrap();
    let bounds = cx.debug_bounds("enable-discovery").unwrap();
    assert!(bounds.left() >= px(16.) && bounds.right() <= px(212.));
    assert!(bounds.bottom() <= px(600.));
    cx.simulate_mouse_move(bounds.center(), None, Default::default());
    cx.simulate_click(bounds.center(), Default::default());
    cx.run_until_parked();
    cx.update(|_, cx| {
        let state = view.read(cx);
        assert_eq!(state.engine.settings().discovery, before.discovery);
        assert!(matches!(state.status_tone, StatusTone::Err));
    });
    assert!(cx.debug_bounds("enable-discovery").is_some());

    std::fs::remove_dir(paths.settings_file()).unwrap();
    before.save(&paths).unwrap();
    let bounds = cx.debug_bounds("enable-discovery").unwrap();
    cx.simulate_mouse_move(bounds.center(), None, Default::default());
    cx.simulate_click(bounds.center(), Default::default());
    cx.run_until_parked();
    let saved = removent_core::Settings::load(&paths).unwrap();
    assert!(saved.discovery.removent && saved.discovery.vnc && saved.discovery.rdp);
    assert_eq!(saved.host_enabled, before.host_enabled);
    assert_eq!(saved.vnc_enabled, before.vnc_enabled);
    assert!(cx.debug_bounds("enable-discovery").is_none());
    cx.update(|_, cx| {
        let state = view.read(cx);
        assert_eq!(state.engine.settings().discovery, saved.discovery);
        assert!(matches!(state.status_tone, StatusTone::Ok));
        assert!(!state.settings_open);
    });
}

#[gpui::test]
fn settings_sidebar_switches_sections_and_preserves_device_selection(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let directory = tempfile::tempdir().unwrap();
    let paths = removent_core::DataPaths {
        root: directory.path().into(),
    };
    let (commands, _rx) = tokio::sync::mpsc::channel(1);
    let engine = Engine::for_ui_test(paths, commands);
    let slot = Rc::new(std::cell::RefCell::new(None));
    let out = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let view = cx.new(|cx| HomeView::new(engine, window, cx));
        view.update(cx, |view, _| {
            view.devices.insert(
                "office".into(),
                DeviceRow {
                    name: "Office Mac".into(),
                    hosts: vec![],
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
    let engine = Engine::for_ui_test(paths.clone(), commands);
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
    let engine = Engine::for_ui_test(paths.clone(), commands);
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
fn update_action_stays_above_long_release_notes_in_both_themes(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let directory = tempfile::tempdir().unwrap();
    let paths = removent_core::DataPaths {
        root: directory.path().into(),
    };
    let (commands, _rx) = tokio::sync::mpsc::channel(1);
    let engine = Engine::for_ui_test(paths, commands);
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let view = cx.new(|cx| HomeView::new(engine, window, cx));
        view.update(cx, |view, _| {
            view.settings_open = true;
            view.settings_section = 4;
            view.update_status = UpdateStatus::Available {
                version: "9.0.0".into(),
                notes: "修复连接恢复，改进远程画面与后台服务。\n".repeat(80),
            };
        });
        gpui_component::Root::new(AnyView::from(view), window, cx)
    });
    for dark in [false, true] {
        cx.update(|window, cx| {
            let mode = if dark {
                gpui_component::ThemeMode::Dark
            } else {
                gpui_component::ThemeMode::Light
            };
            gpui_component::Theme::change(mode, Some(window), cx);
            crate::theme::apply(dark, cx);
        });
        for size in [
            gpui::size(px(860.), px(600.)),
            gpui::size(px(1040.), px(700.)),
        ] {
            cx.simulate_resize(size);
            cx.run_until_parked();
            let button = cx
                .debug_bounds("update-download")
                .expect("update action renders");
            let notes = cx
                .debug_bounds("update-release-notes")
                .expect("release notes render");
            assert!(
                button.bottom() <= notes.top(),
                "action must precede the release notes"
            );
            assert!(button.right() <= size.width);
            assert!(
                button.bottom() <= size.height,
                "action must be visible without scrolling"
            );
        }
    }
}

#[gpui::test]
fn discovery_switches_persist_independently_and_remove_disabled_rows(cx: &mut TestAppContext) {
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
                    hosts: vec![],
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
fn no_update_result_only_renders_for_manual_checks_while_history_remains_visible(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_component::init);
    let directory = tempfile::tempdir().unwrap();
    let paths = removent_core::DataPaths {
        root: directory.path().into(),
    };
    std::fs::write(
        paths.root.join("update-check.json"),
        r#"{"last_checked_at":1791829440}"#,
    )
    .unwrap();
    let (commands, _rx) = tokio::sync::mpsc::channel(1);
    let engine = Engine::for_ui_test(paths, commands);
    assert_eq!(engine.last_update_check(), Some(1791829440));
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
    for dark in [false, true] {
        cx.update(|window, cx| {
            gpui_component::Theme::change(
                if dark {
                    gpui_component::ThemeMode::Dark
                } else {
                    gpui_component::ThemeMode::Light
                },
                Some(window),
                cx,
            );
            crate::theme::apply(dark, cx);
        });
        for (status, result, available) in [
            (UpdateStatus::Idle, false, false),
            (UpdateStatus::UpToDate { manual: false }, false, false),
            (UpdateStatus::UpToDate { manual: true }, true, false),
            (
                UpdateStatus::Available {
                    version: "9.0.0".into(),
                    notes: String::new(),
                },
                false,
                true,
            ),
        ] {
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.update_status = status;
                    cx.notify();
                })
            });
            cx.run_until_parked();
            assert_eq!(cx.debug_bounds("update-up-to-date").is_some(), result);
            assert_eq!(cx.debug_bounds("update-download").is_some(), available);
            let time = cx
                .debug_bounds("update-last-check")
                .expect("historical check time remains visible");
            assert!(time.right() <= px(860.) && time.bottom() <= px(700.));
        }
    }
}

#[gpui::test]
fn saved_connections_hide_nearby_duplicates_and_reappear_after_bookmark_removal(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_component::init);
    let directory = tempfile::tempdir().unwrap();
    let paths = removent_core::DataPaths {
        root: directory.path().into(),
    };
    let (commands, _rx) = tokio::sync::mpsc::channel(1);
    let engine = Engine::for_ui_test(paths, commands);
    engine.update_settings(|s| s.discovery.vnc = true).unwrap();
    let slot = Rc::new(std::cell::RefCell::new(None));
    let out = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let view = cx.new(|cx| HomeView::new(engine, window, cx));
        view.update(cx, |view, cx| {
            view.handle_event(
                UiEvent::DeviceFound {
                    fp: "office".into(),
                    name: "Mac mini".into(),
                    addr: "192.168.1.11:5900".parse().unwrap(),
                    hosts: vec!["mac-mini.local.".into(), "2001:db8::1".into()],
                    protocol: ConnectionProtocol::Vnc,
                },
                window,
                cx,
            );
            view.selected = Some(Selection::Device("office".into()));
        });
        *out.borrow_mut() = Some(view.clone());
        gpui_component::Root::new(AnyView::from(view), window, cx)
    });
    let view = slot.borrow_mut().take().unwrap();
    cx.simulate_resize(gpui::size(px(1040.), px(700.)));
    let saved = SavedConnection {
        id: "saved-office".into(),
        name: "br-gateway".into(),
        host: "192.168.1.11".into(),
        port: 5900,
        protocol: ConnectionProtocol::Vnc,
        ..Default::default()
    };
    for (entries, discovered) in [
        (vec![saved.clone()], false),
        (vec![], true),
        (
            vec![SavedConnection {
                port: 5901,
                ..saved.clone()
            }],
            true,
        ),
        (
            vec![SavedConnection {
                protocol: ConnectionProtocol::Rdp,
                ..saved.clone()
            }],
            true,
        ),
        (
            vec![SavedConnection {
                host: "MAC-MINI.LOCAL.".into(),
                ..saved.clone()
            }],
            false,
        ),
        (
            vec![SavedConnection {
                host: "[2001:0db8::1]".into(),
                ..saved.clone()
            }],
            false,
        ),
        (vec![], true),
    ] {
        let saved_visible = !entries.is_empty();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.handle_event(
                    UiEvent::CloudSync {
                        status: serde_json::json!({"code":"off"}),
                        entries,
                    },
                    window,
                    cx,
                );
                assert!(
                    view.devices.contains_key("office"),
                    "deduplication retains the live discovery source"
                );
                if !discovered {
                    assert!(
                        matches!(&view.selected, Some(Selection::Saved(id)) if id == "saved-office")
                    );
                }
            })
        });
        cx.run_until_parked();
        assert_eq!(cx.debug_bounds("discovered-office").is_some(), discovered);
        assert_eq!(
            cx.debug_bounds("bookmark-saved-office").is_some(),
            saved_visible
        );
        if discovered {
            cx.update(|_, cx| {
                view.update(cx, |view, _| {
                    view.selected = Some(Selection::Device("office".into()))
                })
            });
        }
    }
}
