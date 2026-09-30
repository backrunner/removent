use super::*;
use gpui::{AnyView, KeyBinding, TestAppContext};
use gpui_component::Root;
use std::cell::RefCell;
use std::rc::Rc;

struct TestSurface(Entity<ConnectionDialog>);
impl Render for TestSurface {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(Button::new("background-control").label("Background"))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(div().w(px(480.)).child(self.0.clone())),
            )
    }
}

fn setup(cx: &mut TestAppContext) -> (Entity<ConnectionDialog>, &mut gpui::VisualTestContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.bind_keys([
            KeyBinding::new("tab", ConnectionTab, Some("ConnectionDialog")),
            KeyBinding::new("shift-tab", ConnectionTabPrev, Some("ConnectionDialog")),
        ]);
    });
    let slot = Rc::new(RefCell::new(None));
    let out = slot.clone();
    let (_, cx) = cx.add_window_view(|window, cx| {
        let dialog = cx.new(|cx| ConnectionDialog::new(window, cx));
        *out.borrow_mut() = Some(dialog.clone());
        let surface = cx.new(|_| TestSurface(dialog));
        Root::new(AnyView::from(surface), window, cx)
    });
    let dialog = slot.borrow_mut().take().unwrap();
    (dialog, cx)
}

fn relay_bookmark() -> SavedConnection {
    SavedConnection {
        id: "relay-bookmark".into(),
        protocol: ConnectionProtocol::Removent,
        host: "office".into(),
        port: 0,
        relay: Some(
            RelayRoute::parse(
                "removent://relay.example:443",
                RelayTransport::WebSocket,
                "",
                &"aa".repeat(32),
            )
            .unwrap(),
        ),
        password_hint: true,
        ..Default::default()
    }
}

#[gpui::test]
fn switching_carrier_clears_credentials_and_requires_its_trust(cx: &mut TestAppContext) {
    let (dialog, cx) = setup(cx);
    cx.update(|window, cx| {
        dialog.update(cx, |form, cx| {
            form.prefill(&relay_bookmark(), Some("11".repeat(32)), window, cx);
        })
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        dialog.update(cx, |form, cx| {
            form.choose_relay_transport(RelayTransport::Quic, window, cx);
            assert!(form.password.read(cx).value().is_empty());
            assert!(form.request(cx).is_err(), "QUIC needs its own relay pin");
            form.relay_pin
                .update(cx, |s, cx| s.set_value("bb".repeat(32), window, cx));
            let request = form.request(cx).unwrap();
            assert_eq!(request.relay.unwrap().transport, RelayTransport::Quic);
            form.password
                .update(cx, |s, cx| s.set_value("22".repeat(32), window, cx));
            form.choose_relay_transport(RelayTransport::WebSocket, window, cx);
            assert!(form.password.read(cx).value().is_empty());
            let request = form.request(cx).unwrap();
            assert!(request.relay.unwrap().server_fingerprint.is_empty());
        })
    });
}

#[gpui::test]
fn relay_prefill_preserves_secret_but_destination_edits_clear_it(cx: &mut TestAppContext) {
    let (dialog, cx) = setup(cx);
    cx.update(|window, cx| {
        dialog.update(cx, |form, cx| {
            form.prefill(&relay_bookmark(), Some("11".repeat(32)), window, cx)
        })
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        let form = dialog.read(cx);
        assert!(form.via_relay);
        let request = form.request(cx).unwrap();
        assert_eq!(request.password, "11".repeat(32));
        assert_eq!(request.address.host, "office");
        assert_eq!(request.relay, relay_bookmark().relay);
    });
    cx.update(|window, cx| {
        dialog.update(cx, |form, cx| {
            form.relay_endpoint.update(cx, |s, cx| {
                s.set_value("removent://another.example:443", window, cx)
            });
        })
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        dialog.update(cx, |form, cx| {
            assert!(form.password.read(cx).value().is_empty());
            assert!(
                form.request(cx).unwrap().password.is_empty(),
                "credential-free registration is valid"
            );
            form.password
                .update(cx, |s, cx| s.set_value("22".repeat(32), window, cx));
        })
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        dialog.update(cx, |form, cx| {
            form.host
                .update(cx, |s, cx| s.set_value("other-room", window, cx));
        })
    });
    cx.run_until_parked();
    cx.update(|_, cx| assert!(dialog.read(cx).password.read(cx).value().is_empty()));
}

#[gpui::test]
fn relay_selection_does_not_reuse_a_previous_routes_credential(cx: &mut TestAppContext) {
    let (dialog, cx) = setup(cx);
    cx.update(|window, cx| {
        dialog.update(cx, |form, cx| {
            form.prefill(&relay_bookmark(), Some("11".repeat(32)), window, cx)
        })
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        dialog.update(cx, |form, cx| {
            let mut other = relay_bookmark();
            other.password_hint = false;
            other.relay.as_mut().unwrap().endpoint = "removent://second.example:443".into();
            form.select_relay(&other, window, cx);
        })
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        let request = dialog.read(cx).request(cx).unwrap();
        assert!(request.password.is_empty());
        assert_eq!(
            request.relay.unwrap().endpoint,
            "removent://second.example:443"
        );
    });
}

#[gpui::test]
fn relay_form_scrolls_and_keeps_actions_visible(cx: &mut TestAppContext) {
    let (dialog, cx) = setup(cx);
    cx.simulate_resize(gpui::size(px(860.), px(600.)));
    cx.update(|window, cx| {
        dialog.update(cx, |form, cx| {
            form.set_relay_choices(vec![relay_bookmark()]);
            form.prefill(&relay_bookmark(), None, window, cx);
            form.port.update(cx, |s, cx| s.set_value("0", window, cx));
            form.save(cx);
            assert!(
                form.error.is_none(),
                "relay routes must ignore the hidden direct port"
            );
        })
    });
    cx.run_until_parked();
    let fields = cx.debug_bounds("connection-fields").unwrap();
    let footer = cx.debug_bounds("connection-footer").unwrap();
    assert!(fields.size.height > px(100.));
    assert!(footer.top() >= fields.bottom());
    assert!(footer.bottom() <= px(600.));
    for _ in 0..20 {
        cx.simulate_keystrokes("tab");
        cx.update(|window, cx| assert!(dialog.read(cx).focus.contains_focused(window, cx)));
    }
}

#[gpui::test]
fn discovered_compatibility_target_prefills_without_trust_or_credentials(cx: &mut TestAppContext) {
    let (dialog, cx) = setup(cx);
    cx.update(|window, cx| {
        dialog.update(cx, |form, cx| {
            for protocol in [ConnectionProtocol::Vnc, ConnectionProtocol::Rdp] {
                form.prefill_discovered(
                    protocol,
                    "Office".into(),
                    "[fe80::1%3]:3391".parse().unwrap(),
                    window,
                    cx,
                );
                assert!(form.step == Step::Details(protocol));
                assert_eq!(form.memo_name(cx), "Office");
                assert!(form.saved_id().is_none());
                assert!(form.username.read(cx).value().is_empty());
                assert!(form.password.read(cx).value().is_empty());
                if protocol == ConnectionProtocol::Rdp {
                    assert!(form.request(cx).is_err());
                    form.username
                        .update(cx, |s, cx| s.set_value("alice", window, cx));
                }
                let request = form.request(cx).unwrap();
                assert_eq!(request.protocol, protocol);
                assert_eq!(request.address.to_string(), "[fe80::1%3]:3391");
                assert!(!request.accept_invalid_certificate);
                assert!(request.relay.is_none());
            }
        });
    });
}

#[gpui::test]
fn switching_protocol_clears_credentials_and_uses_correct_port(cx: &mut TestAppContext) {
    let (dialog, cx) = setup(cx);
    cx.update(|window, cx| {
        dialog.update(cx, |form, cx| {
            form.choose(ConnectionProtocol::Rdp, window, cx);
            form.host
                .update(cx, |s, cx| s.set_value("office.local", window, cx));
            form.username
                .update(cx, |s, cx| s.set_value("alice", window, cx));
            form.password
                .update(cx, |s, cx| s.set_value("private", window, cx));
            form.domain
                .update(cx, |s, cx| s.set_value("office", window, cx));
            form.accept_invalid_certificate = true;
            assert_eq!(form.request(cx).unwrap().address.port, 3389);
            form.back(window, cx);
            assert!(form.password.read(cx).value().is_empty());
            form.choose(ConnectionProtocol::Vnc, window, cx);
            let request = form.request(cx).unwrap();
            assert_eq!(request.address.port, 5900);
            assert!(
                request.username.is_empty()
                    && request.password.is_empty()
                    && request.domain.is_empty()
            );
            assert!(!request.accept_invalid_certificate);
        })
    });
}

#[gpui::test]
fn enter_validates_and_failure_allows_editing_and_retry(cx: &mut TestAppContext) {
    let (dialog, cx) = setup(cx);
    cx.update(|window, cx| {
        dialog.update(cx, |form, cx| {
            form.choose(ConnectionProtocol::Rdp, window, cx);
        })
    });
    cx.simulate_keystrokes("enter");
    cx.update(|window, cx| {
        dialog.update(cx, |form, cx| {
            assert!(form.error.is_some());
            form.host
                .update(cx, |s, cx| s.set_value("192.168.1.10", window, cx));
            assert!(form.request(cx).is_err()); // RDP username is required.
            form.username
                .update(cx, |s, cx| s.set_value("alice", window, cx));
            form.port
                .update(cx, |s, cx| s.set_value("3390", window, cx));
            assert_eq!(form.request(cx).unwrap().address.port, 3390);
            form.set_connecting(true, None, cx);
            form.set_connecting(false, Some("Authentication failed".into()), cx);
            assert_eq!(form.host.read(cx).value().as_str(), "192.168.1.10");
            assert!(form.request(cx).is_ok());
        })
    });
}

#[gpui::test]
fn bookmark_edit_keeps_identity_and_can_save_without_connecting(cx: &mut TestAppContext) {
    let (dialog, cx) = setup(cx);
    let mut events = cx.events(&dialog);
    cx.update(|window, cx| {
        dialog.update(cx, |form, cx| {
            let saved = SavedConnection {
                id: "stable-bookmark".into(),
                protocol: ConnectionProtocol::Vnc,
                host: "old.local".into(),
                port: 5900,
                ..Default::default()
            };
            form.prefill(&saved, None, window, cx);
            form.host
                .update(cx, |s, cx| s.set_value("new.local", window, cx));
            form.save(cx);
            assert_eq!(form.saved_id(), Some("stable-bookmark"));
            assert!(!form.connecting);
            form.set_save_warning(Some("Keychain unavailable".into()), cx);
            form.set_connecting(true, None, cx);
            form.set_stage(ConnectionStage::PreparingDesktop, cx);
            assert_eq!(form.save_warning.as_deref(), Some("Keychain unavailable"));
        })
    });
    cx.run_until_parked();
    assert_eq!(events.try_recv().unwrap(), ConnectionDialogEvent::Save);
}

#[gpui::test]
fn keyboard_focus_stays_in_the_dialog_at_minimum_window_size(cx: &mut TestAppContext) {
    let (dialog, cx) = setup(cx);
    cx.simulate_resize(gpui::size(px(860.), px(600.)));
    cx.update(|window, cx| {
        dialog.update(cx, |form, cx| {
            form.choose(ConnectionProtocol::Rdp, window, cx)
        })
    });
    for _ in 0..18 {
        cx.simulate_keystrokes("tab");
        cx.update(|window, cx| assert!(dialog.read(cx).focus.contains_focused(window, cx)));
    }
    for _ in 0..18 {
        cx.simulate_keystrokes("shift-tab");
        cx.update(|window, cx| assert!(dialog.read(cx).focus.contains_focused(window, cx)));
    }
}

#[gpui::test]
fn connection_footer_stays_visible_at_minimum_size(cx: &mut TestAppContext) {
    let (dialog, cx) = setup(cx);
    cx.simulate_resize(gpui::size(px(860.), px(600.)));
    cx.update(|window, cx| {
        dialog.update(cx, |form, cx| {
            form.choose(ConnectionProtocol::Rdp, window, cx);
            form.set_connecting(true, None, cx);
        })
    });
    cx.run_until_parked();
    let fields = cx.debug_bounds("connection-fields").unwrap();
    let footer = cx.debug_bounds("connection-footer").unwrap();
    assert!(fields.size.height > px(100.));
    assert!(footer.top() >= fields.bottom());
    assert!(footer.bottom() <= px(600.));
    cx.update(|_, cx| {
        dialog.update(cx, |form, cx| {
            form.set_connecting(
                false,
                Some(
                    "The remote device refused the connection. Check the address and try again."
                        .into(),
                ),
                cx,
            );
        })
    });
    cx.run_until_parked();
    let footer = cx.debug_bounds("connection-footer").unwrap();
    assert!(footer.bottom() <= px(600.));
}

#[gpui::test]
fn back_returns_one_level_and_cancels_a_pending_connection(cx: &mut TestAppContext) {
    let (dialog, cx) = setup(cx);
    let mut events = cx.events(&dialog);
    cx.update(|window, cx| {
        dialog.update(cx, |form, cx| {
            form.choose(ConnectionProtocol::Rdp, window, cx);
            form.back(window, cx);
            assert!(matches!(form.step, Step::Protocol));
        })
    });
    assert!(events.try_recv().is_err());
    cx.update(|window, cx| dialog.update(cx, |form, cx| form.back(window, cx)));
    assert_eq!(events.try_recv().unwrap(), ConnectionDialogEvent::Close);
    cx.update(|window, cx| {
        dialog.update(cx, |form, cx| {
            form.choose(ConnectionProtocol::Rdp, window, cx);
            form.set_connecting(true, None, cx);
            form.back(window, cx);
        })
    });
    assert_eq!(events.try_recv().unwrap(), ConnectionDialogEvent::Cancel);
}

#[gpui::test]
fn submit_is_guarded_and_cancel_preserves_details_for_retry(cx: &mut TestAppContext) {
    let (dialog, cx) = setup(cx);
    let mut events = cx.events(&dialog);
    cx.update(|window, cx| {
        dialog.update(cx, |form, cx| {
            form.choose(ConnectionProtocol::Vnc, window, cx);
            form.host
                .update(cx, |s, cx| s.set_value("192.168.1.11", window, cx));
            form.password
                .update(cx, |s, cx| s.set_value("secret", window, cx));
            form.submit(cx);
            form.submit(cx);
            assert!(form.connecting);
            assert!(form.elapsed_task.is_some());
            form.set_stage(ConnectionStage::Negotiating, cx);
            assert_eq!(form.stage, ConnectionStage::Negotiating);
        })
    });
    assert_eq!(events.try_recv().unwrap(), ConnectionDialogEvent::Submit);
    assert!(events.try_recv().is_err());
    cx.update(|_, cx| {
        dialog.update(cx, |form, cx| {
            form.cancel(cx);
            assert!(form.cancelled && !form.connecting);
            assert!(form.elapsed_task.is_none());
            assert_eq!(form.request(cx).unwrap().password, "secret");
            form.set_stage(ConnectionStage::PreparingDesktop, cx);
            assert_eq!(form.stage, ConnectionStage::Negotiating);
            form.submit(cx);
            assert!(form.connecting && !form.cancelled);
            assert_eq!(form.stage, ConnectionStage::Resolving);
            form.set_connecting(false, Some("Authentication failed".into()), cx);
            assert!(form.retry);
            assert!(form.elapsed_task.is_none());
        })
    });
    assert_eq!(events.try_recv().unwrap(), ConnectionDialogEvent::Submit);
}

#[gpui::test]
fn covered_form_does_not_receive_keys_or_submit_and_restores_focus(cx: &mut TestAppContext) {
    let (dialog, cx) = setup(cx);
    let mut events = cx.events(&dialog);
    let fallback = cx.update(|window, cx| {
        let fallback = cx.focus_handle();
        dialog.update(cx, |form, cx| {
            form.choose(ConnectionProtocol::Vnc, window, cx);
            form.host
                .update(cx, |s, cx| s.set_value("localhost", window, cx));
            form.set_obscured(true, &fallback, window, cx);
            form.submit(cx);
            form.back(window, cx);
            assert!(!form.focus.contains_focused(window, cx));
            assert!(matches!(form.step, Step::Details(ConnectionProtocol::Vnc)));
        });
        fallback
    });
    assert!(events.try_recv().is_err());
    cx.update(|window, cx| {
        dialog.update(cx, |form, cx| {
            form.set_obscured(false, &fallback, window, cx);
            assert!(form.host.focus_handle(cx).is_focused(window));
            assert_eq!(form.host.read(cx).value().as_str(), "localhost");
        })
    });
}
