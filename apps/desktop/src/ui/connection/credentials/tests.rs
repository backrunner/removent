use super::*;
use crate::ui::connection::tests::setup;

fn credentials() -> Vec<NativeCredential> {
    vec![
        NativeCredential::fixture(
            AuthenticationMode::SystemAccount,
            removent_proto::SystemCredentials {
                username: "owner".into(),
                password: "system-secret".into(),
            }
            .encode(),
        ),
        NativeCredential::fixture(AuthenticationMode::Password, "access-secret".into()),
    ]
}

#[gpui::test]
fn edits_are_validated_and_never_follow_a_changed_destination(cx: &mut gpui::TestAppContext) {
    let (dialog, cx) = setup(cx);
    dialog.update_in(cx, |form, window, cx| {
        let saved = SavedConnection::from_request(
            &ConnectionRequest::native("127.0.0.1:48688".parse().unwrap()),
            "Fixture",
        );
        form.prefill(&saved, None, window, cx);
        form.set_native_credentials(credentials(), window, cx);
        assert!(form.native_credential_edits(cx).unwrap().is_empty());
        assert!(form.request(cx).unwrap().password.is_empty());
        form.native_credentials[0]
            .username
            .update(cx, |s, cx| s.set_value("new-owner", window, cx));
        form.native_credentials[0]
            .password
            .update(cx, |s, cx| s.set_value("new-system-secret", window, cx));
        form.native_credentials[1]
            .password
            .update(cx, |s, cx| s.set_value("new-access-secret", window, cx));
        let edits = form.native_credential_edits(cx).unwrap();
        assert_eq!(edits.len(), 2);
        let system = removent_proto::SystemCredentials::decode(&edits[0].answer.value).unwrap();
        assert_eq!(system.username, "new-owner");
        assert_eq!(system.password, "new-system-secret");
        assert_eq!(edits[1].answer.value, "new-access-secret");
        assert!(
            form.request(cx).unwrap().password.is_empty(),
            "native secrets never become relay tokens"
        );
        form.native_credentials[0]
            .password
            .update(cx, |s, cx| s.set_value("", window, cx));
        assert!(form.request(cx).is_err());
        form.native_credentials[0].remember = false;
        assert!(form.request(cx).is_ok());
        assert!(!form.native_credential_edits(cx).unwrap()[0].answer.remember);
        form.host
            .update(cx, |s, cx| s.set_value("127.0.0.2", window, cx));
        assert!(form.native_credential_edits(cx).unwrap().is_empty());
        form.choose(ConnectionProtocol::Vnc, window, cx);
        assert!(form.native_credentials.is_empty());
        assert!(form.request(cx).unwrap().password.is_empty());
    });
}

#[gpui::test]
fn native_credential_form_scrolls_with_actions_visible_in_both_themes(
    cx: &mut gpui::TestAppContext,
) {
    let (dialog, cx) = setup(cx);
    cx.simulate_resize(gpui::size(px(860.), px(600.)));
    dialog.update_in(cx, |form, window, cx| {
        let saved = SavedConnection::from_request(
            &ConnectionRequest::native("127.0.0.1:48688".parse().unwrap()),
            "Fixture",
        );
        form.prefill(&saved, None, window, cx);
        form.set_native_credentials(credentials(), window, cx);
    });
    for dark in [true, false] {
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
        cx.run_until_parked();
        let fields = cx.debug_bounds("connection-fields").unwrap();
        let footer = cx.debug_bounds("connection-footer").unwrap();
        assert!(fields.size.height > px(100.));
        assert!(footer.top() >= fields.bottom());
        assert!(footer.bottom() <= px(600.));
        let credential = cx.debug_bounds("native-credential-0").unwrap();
        assert!(credential.left() >= fields.left());
        assert!(credential.right() <= fields.right());
        for _ in 0..24 {
            cx.simulate_keystrokes("tab");
            cx.update(|window, cx| assert!(dialog.read(cx).focus.contains_focused(window, cx)));
        }
    }
}
