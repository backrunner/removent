use super::*;
use gpui::{AnyView, TestAppContext};

#[gpui::test]
fn right_click_opens_configuration_for_the_clicked_target_at_minimum_size(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let directory = tempfile::tempdir().unwrap();
    let paths = removent_core::DataPaths {
        root: directory.path().into(),
    };
    let mut store = removent_client::saved::SavedConnections::load(&paths).unwrap();
    let first = store
        .upsert(SavedConnection {
            name: "First".into(),
            host: "first.local".into(),
            port: 5900,
            protocol: ConnectionProtocol::Vnc,
            ..Default::default()
        })
        .unwrap();
    let second = store
        .upsert(SavedConnection {
            name: "Second".into(),
            host: "second.local".into(),
            port: 5900,
            protocol: ConnectionProtocol::Vnc,
            ..Default::default()
        })
        .unwrap();
    let (commands, _rx) = tokio::sync::mpsc::channel(1);
    let engine = Engine::for_ui_test(paths.clone(), commands);
    let slot = Rc::new(std::cell::RefCell::new(None));
    let out = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let view = cx.new(|cx| HomeView::new(engine, window, cx));
        view.update(cx, |view, _| {
            view.selected = Some(Selection::Saved(first.id))
        });
        *out.borrow_mut() = Some(view.clone());
        gpui_component::Root::new(AnyView::from(view), window, cx)
    });
    let _home = slot.borrow_mut().take().unwrap();
    cx.simulate_resize(gpui::size(px(860.), px(600.)));
    cx.run_until_parked();
    let selector: &'static str = Box::leak(format!("bookmark-{}", second.id).into_boxed_str());
    let row = cx.debug_bounds(selector).unwrap();
    let press = row.center();
    cx.simulate_mouse_down(press, MouseButton::Right, Default::default());
    cx.simulate_mouse_up(press, MouseButton::Right, Default::default());
    cx.run_until_parked();
    let item = gpui::point(press.x + px(30.), press.y + px(17.));
    cx.simulate_mouse_move(item, None, Default::default());
    cx.simulate_click(item, Default::default());
    cx.run_until_parked();
    let target_selector: &'static str =
        Box::leak(format!("wol-target-{}", second.id).into_boxed_str());
    assert!(cx.debug_bounds(target_selector).is_some());
    for selector in ["wol-mac", "wol-destination", "wol-port", "wol-footer"] {
        let bounds = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("missing {selector}"));
        assert!(bounds.bottom() <= px(600.), "{selector} is clipped");
    }
    cx.update(|window, cx| assert!(window.has_active_dialog(cx)));
    // Saving with an empty MAC must leave the dialog open, with a useful error.
    let bounds = cx.debug_bounds("wol-save-button").unwrap();
    cx.simulate_click(bounds.center(), Default::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("wol-error").is_some());
    // Dialog confirmation (Enter) must use the same validation instead of
    // dismissing an unsaved form through the component's default handler.
    let input = cx.debug_bounds("wol-mac").unwrap();
    cx.simulate_click(input.center(), Default::default());
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(cx.debug_bounds("wol-error").is_some());
    cx.update(|window, cx| assert!(window.has_active_dialog(cx)));
    assert!(
        removent_client::saved::SavedConnections::load(&paths)
            .unwrap()
            .all()
            .iter()
            .all(|entry| entry.wake_on_lan.is_none())
    );
}

#[gpui::test]
fn discovered_ipv6_wake_bookmarks_and_rdp_forms_keep_interface_scope(cx: &mut TestAppContext) {
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
        let home = cx.new(|cx| HomeView::new(engine, window, cx));
        *out.borrow_mut() = Some(home.clone());
        gpui_component::Root::new(AnyView::from(home), window, cx)
    });
    let home = slot.borrow_mut().take().unwrap();
    cx.update(|window, cx| {
        home.update(cx, |home, cx| {
            for protocol in [
                ConnectionProtocol::Removent,
                ConnectionProtocol::Vnc,
                ConnectionProtocol::Rdp,
            ] {
                let row = DeviceRow {
                    name: "Scoped computer".into(),
                    addr: "[fe80::1234%8]:5900".parse().unwrap(),
                    hosts: vec![],
                    protocol,
                };
                let target = home.discovered_wake_target(&row);
                assert_eq!(target.host, "fe80::1234%8");
                if protocol == ConnectionProtocol::Rdp {
                    home.open_wake_settings(target, window, cx);
                } else {
                    let config = WakeOnLan::parse("021122334455", "255.255.255.255", "9").unwrap();
                    let saved = home.engine.set_wake_on_lan(&target, Some(config)).unwrap();
                    assert_eq!(saved.address().to_string(), "[fe80::1234%8]:5900");
                }
            }
        });
    });

    cx.run_until_parked();
    cx.simulate_input("owner");
    cx.update(|_, cx| {
        let form = home.read(cx).connection_dialog.as_ref().unwrap().read(cx);
        let request = form.request(cx).unwrap();
        assert_eq!(request.username, "owner");
        assert_eq!(request.address.to_string(), "[fe80::1234%8]:5900");
    });
}
