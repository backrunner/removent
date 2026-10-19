use super::*;

#[cfg(target_os = "macos")]
#[gpui::test]
fn remembered_credentials_can_be_reviewed_and_forgotten_without_leaking_between_modes(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_component::init);
    let directory = tempfile::tempdir().unwrap();
    let (commands, _rx) = tokio::sync::mpsc::channel(1);
    let engine = Engine::for_ui_test(
        removent_core::DataPaths {
            root: directory.path().into(),
        },
        commands,
    );
    let slot = Rc::new(std::cell::RefCell::new(None));
    let out = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let view = cx.new(|cx| HomeView::new(engine, window, cx));
        *out.borrow_mut() = Some(view.clone());
        gpui_component::Root::new(AnyView::from(view), window, cx)
    });
    let view = slot.borrow_mut().take().unwrap();
    cx.simulate_resize(gpui::size(px(860.), px(600.)));
    for mode in [
        removent_core::AuthenticationMode::SystemAccount,
        removent_core::AuthenticationMode::Password,
        removent_core::AuthenticationMode::Otp,
    ] {
        let (tx, mut answer) = oneshot::channel();
        let value = if mode == removent_core::AuthenticationMode::SystemAccount {
            removent_proto::SystemCredentials {
                username: "owner".into(),
                password: "test-password".into(),
            }
            .encode()
        } else {
            "test-password".into()
        };
        let expected = value.clone();
        view.update_in(cx, |view, window, cx| {
            view.connecting = Some("auth fixture".into());
            view.handle_event(
                UiEvent::ClientNeedsPin {
                    generation: view.engine.client_generation(),
                    mode,
                    tx,
                    prefill: Some(crate::engine::AuthenticationAnswer {
                        value,
                        remember: true,
                    }),
                },
                window,
                cx,
            );
            assert_eq!(view.connection_stage, ConnectionStage::Authenticating);
            if mode == removent_core::AuthenticationMode::Otp {
                assert!(!view.remember_auth);
                assert!(view.pin_input.read(cx).value().is_empty());
                assert!(view.auth_username_input.read(cx).value().is_empty());
                view.cancel_pin(window, cx);
            }
        });
        cx.run_until_parked();
        if mode == removent_core::AuthenticationMode::Otp {
            assert!(answer.try_recv().is_err());
            continue;
        }
        let toggle = cx.debug_bounds("remember-auth-toggle").unwrap();
        assert!(toggle.right() <= px(860.) && toggle.bottom() <= px(600.));
        cx.simulate_click(toggle.center(), Default::default());
        cx.run_until_parked();
        view.update_in(cx, |view, window, cx| {
            assert!(!view.remember_auth);
            assert_eq!(view.authentication_input(cx), expected);
            view.submit_pin(window, cx);
            assert!(view.pin_input.read(cx).value().is_empty());
            assert!(view.auth_username_input.read(cx).value().is_empty());
        });
        let answer = answer.try_recv().unwrap();
        assert_eq!(answer.value, expected);
        assert!(!answer.remember);
    }
}

fn open_authentication_choices(cx: &mut gpui::VisualTestContext) {
    let bounds = cx.debug_bounds("authentication-picker").unwrap();
    cx.simulate_mouse_move(bounds.center(), None, Default::default());
    cx.simulate_click(bounds.center(), Default::default());
    cx.run_until_parked();
}

#[gpui::test]
fn system_account_prompt_validates_both_fields_and_clears_credentials(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let directory = tempfile::tempdir().unwrap();
    let paths = removent_core::DataPaths {
        root: directory.path().into(),
    };
    let (commands, _rx) = tokio::sync::mpsc::channel(1);
    let engine = Engine::for_ui_test(paths, commands);
    assert_eq!(
        engine.settings().authentication.mode,
        removent_core::AuthenticationMode::SystemAccount
    );
    let slot = Rc::new(std::cell::RefCell::new(None));
    let out = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let view = cx.new(|cx| HomeView::new(engine, window, cx));
        *out.borrow_mut() = Some(view.clone());
        gpui_component::Root::new(AnyView::from(view), window, cx)
    });
    let view = slot.borrow_mut().take().unwrap();
    cx.simulate_resize(gpui::size(px(860.), px(600.)));
    for cancel in [false, true] {
        let (tx, mut answer) = oneshot::channel();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.handle_event(
                    UiEvent::ClientNeedsPin {
                        generation: view.engine.client_generation(),
                        mode: removent_core::AuthenticationMode::SystemAccount,
                        prefill: None,
                        tx,
                    },
                    window,
                    cx,
                );
                assert!(view.auth_username_input.focus_handle(cx).is_focused(window));
                assert!(!view.remember_auth);
                view.submit_pin(window, cx);
                assert!(view.pin_dialog.is_some(), "empty fields cannot submit");
                view.auth_username_input
                    .update(cx, |input, cx| input.set_value("owner", window, cx));
                view.pin_input.update(cx, |input, cx| {
                    input.set_value("synthetic-password", window, cx)
                });
                view.remember_auth = true;
                if cancel {
                    view.cancel_pin(window, cx);
                } else {
                    view.submit_pin(window, cx);
                }
                assert!(view.pin_input.read(cx).value().is_empty());
                assert!(view.auth_username_input.read(cx).value().is_empty());
                assert!(!view.remember_auth);
            })
        });
        if cancel {
            assert!(answer.try_recv().is_err());
        } else {
            let answer = answer.try_recv().unwrap();
            assert_eq!(
                answer.remember,
                crate::engine::can_remember(removent_core::AuthenticationMode::SystemAccount)
            );
            let credentials = removent_proto::SystemCredentials::decode(&answer.value).unwrap();
            assert_eq!(credentials.username, "owner");
            assert_eq!(credentials.password, "synthetic-password");
        }
    }
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
    let engine = Engine::for_ui_test(paths, commands);
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
    assert!(cx.debug_bounds("authentication-password-editor").is_none());
    open_authentication_choices(cx);
    let password = cx.debug_bounds("authentication-2").unwrap();
    cx.simulate_mouse_move(password.center(), None, Default::default());
    cx.simulate_click(password.center(), Default::default());
    cx.run_until_parked();
    assert_eq!(
        removent_core::Settings::load(&read_paths)
            .unwrap()
            .authentication
            .mode,
        removent_core::AuthenticationMode::SystemAccount
    );
    assert!(cx.debug_bounds("authentication-password-editor").is_some());
    assert!(cx.debug_bounds("authentication-pending").is_some());
    cx.update(|window, cx| {
        let state = view.read(cx);
        assert!(state.auth_password_pending);
        assert!(
            state
                .auth_password_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        );
    });
    // Enter on an empty draft reports the error beside the input and cannot activate it.
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(cx.debug_bounds("authentication-error").is_some());
    cx.update(|_, cx| {
        assert_eq!(
            view.read(cx).engine.settings().authentication.mode,
            removent_core::AuthenticationMode::SystemAccount
        );
    });
    // The limit is UTF-8 bytes, not character count; an invalid save button is inert.
    view.update_in(cx, |view, window, cx| {
        view.auth_password_input.update(cx, |input, cx| {
            input.set_value("界".repeat(342), window, cx)
        });
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("authentication-error").is_some());
    let save = cx.debug_bounds("authentication-save").unwrap();
    cx.simulate_click(save.center(), Default::default());
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        removent_core::Settings::load(&read_paths)
            .unwrap()
            .authentication
            .mode,
        removent_core::AuthenticationMode::SystemAccount
    );
    // Exactly 1024 bytes remains valid; the enabled save action commits the draft.
    let valid = format!("{}a", "界".repeat(341));
    view.update_in(cx, |view, window, cx| {
        view.auth_password_input
            .update(cx, |input, cx| input.set_value(valid, window, cx));
    });
    cx.run_until_parked();
    let save = cx.debug_bounds("authentication-save").unwrap();
    cx.simulate_click(save.center(), Default::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("authentication-pending").is_none());
    assert!(cx.debug_bounds("authentication-error").is_none());
    assert_eq!(
        removent_core::Settings::load(&read_paths)
            .unwrap()
            .authentication
            .mode,
        removent_core::AuthenticationMode::Password
    );
    for (selector, mode) in [
        ("authentication-3", removent_core::AuthenticationMode::Otp),
        ("authentication-4", removent_core::AuthenticationMode::None),
        (
            "authentication-1",
            removent_core::AuthenticationMode::PairingCode,
        ),
    ] {
        cx.run_until_parked();
        open_authentication_choices(cx);
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
fn authentication_password_drafts_restore_saved_values_and_survive_save_failure(
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
        .update_settings(|s| s.authentication.password = "previous-test-password".into())
        .unwrap();
    let before = engine.settings();
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
    open_authentication_choices(cx);
    let password = cx.debug_bounds("authentication-2").unwrap();
    cx.simulate_click(password.center(), Default::default());
    view.update_in(cx, |view, window, cx| {
        view.auth_password_input.update(cx, |input, cx| {
            input.set_value("abandoned-test-draft", window, cx)
        });
    });
    cx.run_until_parked();
    let general = cx.debug_bounds("settings-nav-0").unwrap();
    cx.simulate_click(general.center(), Default::default());
    cx.run_until_parked();
    let security = cx.debug_bounds("settings-nav-2").unwrap();
    cx.simulate_click(security.center(), Default::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("authentication-password-editor").is_none());
    cx.update(|_, cx| {
        let state = view.read(cx);
        assert!(!state.auth_password_pending);
        assert_eq!(
            state.auth_password_input.read(cx).value().as_ref(),
            "previous-test-password"
        );
        assert_eq!(
            state.engine.settings().authentication.mode,
            before.authentication.mode
        );
    });
    // A persistence error retains the draft and the old active mode, permitting retry.
    open_authentication_choices(cx);
    let password = cx.debug_bounds("authentication-2").unwrap();
    cx.simulate_click(password.center(), Default::default());
    view.update_in(cx, |view, window, cx| {
        view.auth_password_input.update(cx, |input, cx| {
            input.set_value("retry-test-password", window, cx)
        });
    });
    std::fs::remove_file(paths.settings_file()).unwrap();
    std::fs::create_dir(paths.settings_file()).unwrap();
    cx.run_until_parked();
    let save = cx.debug_bounds("authentication-save").unwrap();
    cx.simulate_click(save.center(), Default::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("authentication-error").is_some());
    cx.update(|_, cx| {
        let state = view.read(cx);
        assert!(state.auth_password_pending);
        assert_eq!(
            state.auth_password_input.read(cx).value().as_ref(),
            "retry-test-password"
        );
        assert_eq!(
            state.engine.settings().authentication.mode,
            before.authentication.mode
        );
    });
    std::fs::remove_dir(paths.settings_file()).unwrap();
    before.save(&paths).unwrap();
    // Enter retries through the same validation and persistence path as the button.
    view.update_in(cx, |view, window, cx| {
        view.auth_password_input
            .update(cx, |input, cx| input.focus(window, cx))
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let saved = removent_core::Settings::load(&paths).unwrap();
    assert_eq!(
        saved.authentication.mode,
        removent_core::AuthenticationMode::Password
    );
    assert_eq!(saved.authentication.password, "retry-test-password");
    assert!(cx.debug_bounds("authentication-pending").is_none());
    assert!(cx.debug_bounds("authentication-error").is_none());
    // Clearing an active password cannot overwrite the saved value. Closing and
    // reopening settings restores both the active mode and its previous password.
    view.update_in(cx, |view, window, cx| {
        view.auth_password_input
            .update(cx, |input, cx| input.set_value("", window, cx));
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(cx.debug_bounds("authentication-error").is_some());
    let back = cx.debug_bounds("settings-back").unwrap();
    cx.simulate_click(back.center(), Default::default());
    cx.run_until_parked();
    view.update(cx, |view, cx| {
        view.settings_open = true;
        cx.notify();
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("authentication-password-editor").is_some());
    assert!(cx.debug_bounds("authentication-pending").is_none());
    assert!(cx.debug_bounds("authentication-error").is_none());
    cx.update(|_, cx| {
        let state = view.read(cx);
        assert_eq!(
            state.auth_password_input.read(cx).value().as_ref(),
            saved.authentication.password
        );
        assert_eq!(
            state.engine.settings().authentication.mode,
            saved.authentication.mode
        );
    });
    assert_eq!(
        removent_core::Settings::load(&paths)
            .unwrap()
            .authentication
            .password,
        saved.authentication.password
    );
}

#[gpui::test]
fn otp_setup_hides_password_and_secrets_and_resets_reveals_on_navigation(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let directory = tempfile::tempdir().unwrap();
    let paths = removent_core::DataPaths {
        root: directory.path().into(),
    };
    let (commands, _rx) = tokio::sync::mpsc::channel(1);
    let engine = Engine::for_ui_test(paths.clone(), commands);
    engine
        .update_settings(|s| {
            s.authentication.mode = removent_core::AuthenticationMode::Otp;
            s.authentication.otp_secret =
                removent_core::AuthenticationSettings::generate_otp_secret();
        })
        .unwrap();
    let secret = engine.settings().authentication.otp_secret;
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
    assert!(cx.debug_bounds("otp-setup").is_some());
    assert!(cx.debug_bounds("authentication-password-editor").is_none());
    assert!(cx.debug_bounds("otp-secret-visible").is_none());
    assert!(cx.debug_bounds("otp-qr-visible").is_none());
    for selector in ["otp-toggle-qr", "otp-toggle-secret"] {
        let bounds = cx.debug_bounds(selector).unwrap();
        if bounds.bottom() > px(650.) {
            cx.simulate_event(gpui::ScrollWheelEvent {
                position: gpui::point(px(700.), px(500.)),
                delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-250.))),
                touch_phase: gpui::TouchPhase::Moved,
                modifiers: Default::default(),
            });
            cx.run_until_parked();
        }
        let bounds = cx.debug_bounds(selector).unwrap();
        assert!(
            bounds.top() >= px(0.) && bounds.bottom() <= px(700.),
            "enrollment actions must be reachable by scrolling"
        );
        cx.simulate_click(bounds.center(), Default::default());
        cx.run_until_parked();
    }
    assert!(cx.debug_bounds("otp-secret-visible").is_some());
    let qr = cx.debug_bounds("otp-qr-visible").unwrap();
    assert!(qr.right() <= px(860.));
    assert!(qr.size.width >= px(208.));
    cx.update(|_, cx| {
        let state = view.read(cx);
        assert!(state.otp_qr.is_some());
        assert!(!state.otp_setup_error);
    });
    // Hide and reveal the same QR without generating a new authenticator secret.
    let bounds = cx.debug_bounds("otp-toggle-qr").unwrap();
    cx.simulate_click(bounds.center(), Default::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("otp-qr-visible").is_none());
    let bounds = cx.debug_bounds("otp-toggle-qr").unwrap();
    cx.simulate_click(bounds.center(), Default::default());
    cx.run_until_parked();
    let general = cx.debug_bounds("settings-nav-0").unwrap();
    cx.simulate_click(general.center(), Default::default());
    cx.run_until_parked();
    let security = cx.debug_bounds("settings-nav-2").unwrap();
    cx.simulate_click(security.center(), Default::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("otp-setup").is_some());
    assert!(cx.debug_bounds("otp-secret-visible").is_none());
    assert!(cx.debug_bounds("otp-qr-visible").is_none());
    cx.update(|_, cx| {
        let state = view.read(cx);
        assert!(state.otp_qr.is_none());
        assert!(!state.otp_secret_visible);
    });
    open_authentication_choices(cx);
    let password = cx.debug_bounds("authentication-2").unwrap();
    cx.simulate_click(password.center(), Default::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("otp-setup").is_none());
    assert!(cx.debug_bounds("authentication-password-editor").is_some());
    assert_eq!(
        removent_core::Settings::load(&paths)
            .unwrap()
            .authentication
            .otp_secret,
        secret
    );
}

#[gpui::test]
fn certificate_confirmation_requires_a_click_and_rejects_stale_events(cx: &mut TestAppContext) {
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
