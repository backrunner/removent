//! Authentication and admission settings.

use super::*;
use gpui_component::menu::{DropdownMenu, PopupMenuItem};

impl HomeView {
    pub(super) fn render_security_settings(&self, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors;
        let settings = self.engine.settings();
        let row = || div().flex().flex_col().gap_3().p_5().w_full().min_w_0();
        let header = |name, title, description| {
            section_header(name, t!(title).to_string(), t!(description).to_string(), cx)
        };
        let mut content = div().flex().flex_col().gap_5().w_full().min_w_0();
        let committed_mode = settings.authentication.mode;
        let mode = if self.auth_password_pending {
            removent_core::AuthenticationMode::Password
        } else {
            committed_mode
        };
        let password = self.auth_password_input.read(cx).value();
        let password_changed = password.as_ref() != settings.authentication.password;
        let password_error = match &self.auth_password_error {
            Some(AuthenticationFormError::Save(message)) => Some(message.clone()),
            _ if password.len() > 1024 => Some(t!("auth.password_too_long").to_string()),
            Some(AuthenticationFormError::InvalidPassword) if password.is_empty() => {
                Some(t!("auth.password_required").to_string())
            }
            _ => None,
        };
        content = content
            .child(header(
                "shield-check",
                "settings.security",
                "settings.security_description",
            ))
            .child(
                form_group(cx).child(
                    row()
                        .child(setting_label(
                            t!("auth.title").to_string(),
                            t!("auth.hint").to_string(),
                            cx,
                        ))
                        .child({
                            let view = cx.weak_entity();
                            let options = [
                                (removent_core::AuthenticationMode::SystemAccount, "auth.system"),
                                (removent_core::AuthenticationMode::PairingCode, "auth.pairing"),
                                (removent_core::AuthenticationMode::Password, "auth.password"),
                                (removent_core::AuthenticationMode::Otp, "auth.otp"),
                                (removent_core::AuthenticationMode::None, "auth.none"),
                            ];
                            let title = options.iter().find(|(value, _)| *value == mode).unwrap().1;
                            Button::new("authentication-picker")
                                .debug_selector(|| "authentication-picker".into())
                                .outline().w(px(260.)).max_w_full().justify_start()
                                .child(div().flex().items_center().w_full().gap_2()
                                    .child(div().flex_1().child(t!(title).to_string()))
                                    .child(icon_16("chevron-down")))
                                .dropdown_menu(move |mut menu, _, _| {
                                    for (i, (value, key)) in options.into_iter().enumerate() {
                                        let view = view.clone();
                                        menu = menu.item(PopupMenuItem::element(move |_, _| {
                                            div().debug_selector(move || format!("authentication-{i}"))
                                                .child(t!(key).to_string())
                                        }).checked(value == mode).on_click(move |_, window, cx| {
                                            let _ = view.update(cx, |this, cx| this.select_authentication(value, window, cx));
                                        }));
                                    }
                                    menu
                                })
                        })
                        .when(mode == removent_core::AuthenticationMode::SystemAccount, |el| {
                            el.child(div().text_size(px(12.)).text_color(colors.muted_foreground).child(t!("auth.system_hint").to_string()))
                        })
                        .when(mode == removent_core::AuthenticationMode::Password, |el| {
                            let editor = div()
                                .flex()
                                .flex_col()
                                .gap_3()
                                .child(setting_label(
                                    t!("auth.password").to_string(),
                                    t!("auth.password_hint").to_string(),
                                    cx,
                                ))
                                .when(self.auth_password_pending || password_changed, |el| {
                                    let notice = div()
                                        .text_size(px(12.))
                                        .text_color(colors.muted_foreground)
                                        .child(if self.auth_password_pending {
                                            let active = t!(match committed_mode {
                                                removent_core::AuthenticationMode::SystemAccount => "auth.system",
                                                removent_core::AuthenticationMode::PairingCode =>
                                                    "auth.pairing",
                                                removent_core::AuthenticationMode::Password =>
                                                    "auth.password",
                                                removent_core::AuthenticationMode::Otp =>
                                                    "auth.otp",
                                                removent_core::AuthenticationMode::None =>
                                                    "auth.none",
                                            });
                                            t!("auth.password_pending", mode = active).to_string()
                                        } else {
                                            t!("auth.password_unsaved").to_string()
                                        });
                                    #[cfg(test)]
                                    let notice =
                                        notice.debug_selector(|| "authentication-pending".into());
                                    el.child(notice)
                                })
                                .child(
                                    div()
                                        .flex()
                                        .gap_3()
                                        .items_center()
                                        .child(div().flex_1().min_w_0().child(
                                            form_input(&self.auth_password_input).mask_toggle(),
                                        ))
                                        .child({
                                            let save = Button::new("save-auth-password")
                                                .label(
                                                    t!(if self.auth_password_pending {
                                                        "auth.use_password"
                                                    } else {
                                                        "auth.save_password"
                                                    })
                                                    .to_string(),
                                                )
                                                .primary()
                                                .disabled(
                                                    !removent_core::AuthenticationMode::Password
                                                        .valid_input(&password)
                                                        || (!self.auth_password_pending
                                                            && !password_changed),
                                                )
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.save_authentication(
                                                        removent_core::AuthenticationMode::Password,
                                                        cx,
                                                    );
                                                }));
                                            #[cfg(test)]
                                            let save = div()
                                                .debug_selector(|| "authentication-save".into())
                                                .child(save);
                                            save
                                        }),
                                )
                                .when_some(password_error, |el, error| {
                                    let error = div()
                                        .text_size(px(12.))
                                        .text_color(colors.danger)
                                        .child(error);
                                    #[cfg(test)]
                                    let error =
                                        error.debug_selector(|| "authentication-error".into());
                                    el.child(error)
                                });
                            #[cfg(test)]
                            let editor =
                                editor.debug_selector(|| "authentication-password-editor".into());
                            el.child(editor)
                        })
                        .when(mode == removent_core::AuthenticationMode::None, |el| {
                            el.child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(colors.warning)
                                    .child(t!("auth.none_hint").to_string()),
                            )
                        }),
                ),
            );
        if mode == removent_core::AuthenticationMode::Otp {
            content = content.child(self.render_otp_setup(cx));
        }
        if committed_mode == removent_core::AuthenticationMode::PairingCode {
            content = content.child(
                form_group(cx)
                    .child(row()
                        .child(setting_label(t!("pairing.policy").to_string(), t!("pairing.policy_hint").to_string(), cx))
                        .child({
                            let view = cx.entity();
                            segmented(
                                "pairing-policy",
                                &[t!("pairing.remember").to_string(), t!("pairing.every_connection").to_string()],
                                usize::from(settings.authentication.pairing_policy == removent_core::authentication::PairingPolicy::EveryConnection),
                                Rc::new(move |i, _, _, app| {
                                    view.update(app, |this, cx| {
                                        this.persist_settings(|s| {
                                            s.authentication.pairing_policy = if i == 0 {
                                                removent_core::authentication::PairingPolicy::RememberDevice
                                            } else {
                                                removent_core::authentication::PairingPolicy::EveryConnection
                                            };
                                        });
                                        cx.notify();
                                    });
                                }), cx,
                            )
                        }))
                    .child(row()
                        .child(setting_label(t!("pairing.invitation").to_string(), t!("pairing.invitation_hint").to_string(), cx))
                        .child(Button::new("generate-pairing-code")
                            .label(t!("pairing.generate").to_string()).outline()
                            .on_click(cx.listener(|this, _, _, _| this.engine.generate_pairing_code())))),
            );
            content = content.child(
                form_group(cx).child(
                    row()
                        .child(setting_label(
                            t!("settings.admission").to_string(),
                            t!("settings.admission_hint").to_string(),
                            cx,
                        ))
                        .child({
                            let view = cx.entity();
                            segmented(
                                "admission",
                                &[
                                    t!("settings.admission.ask").to_string(),
                                    t!("settings.admission.trusted").to_string(),
                                    t!("settings.admission.deny").to_string(),
                                ],
                                match settings.admission {
                                    AdmissionMode::AlwaysAsk => 0,
                                    AdmissionMode::TrustedAuto => 1,
                                    AdmissionMode::DenyAll => 2,
                                },
                                Rc::new(move |i, _, _, app| {
                                    view.update(app, |this, cx| {
                                        let mode = match i {
                                            1 => AdmissionMode::TrustedAuto,
                                            2 => AdmissionMode::DenyAll,
                                            _ => AdmissionMode::AlwaysAsk,
                                        };
                                        if this.persist_settings(|s| s.admission = mode) {
                                            this.set_status(
                                                t!("status.admission_mode_updated").to_string(),
                                                StatusTone::Ok,
                                            );
                                        }
                                        cx.notify();
                                    });
                                }),
                                cx,
                            )
                        })
                        .child(
                            div()
                                .p_3()
                                .rounded(px(16.))
                                .bg(colors.accent.opacity(0.08))
                                .text_size(px(12.))
                                .text_color(colors.muted_foreground)
                                .child(
                                    t!(match settings.admission {
                                        AdmissionMode::AlwaysAsk => "settings.admission.ask_hint",
                                        AdmissionMode::TrustedAuto =>
                                            "settings.admission.trusted_hint",
                                        AdmissionMode::DenyAll => "settings.admission.deny_hint",
                                    })
                                    .to_string(),
                                ),
                        ),
                ),
            );
        } else {
            content = content.child(
                form_group(cx).child(
                    row()
                        .child(setting_label(
                            t!("auth.allow").to_string(),
                            t!("auth.allow_hint").to_string(),
                            cx,
                        ))
                        .child(
                            Switch::new("allow-authenticated-connections")
                                .checked(settings.admission != AdmissionMode::DenyAll)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.persist_settings(|s| {
                                        s.admission = if s.admission == AdmissionMode::DenyAll {
                                            AdmissionMode::TrustedAuto
                                        } else {
                                            AdmissionMode::DenyAll
                                        }
                                    });
                                    cx.notify();
                                })),
                        ),
                ),
            );
        }
        content
    }
}
