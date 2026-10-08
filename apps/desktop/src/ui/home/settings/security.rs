//! Authentication and admission settings.

use super::*;

impl HomeView {
    pub(super) fn render_security_settings(&self, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors;
        let settings = self.engine.settings();
        let row = || div().flex().flex_col().gap_3().p_5().w_full().min_w_0();
        let header = |name, title, description| {
            section_header(name, t!(title).to_string(), t!(description).to_string(), cx)
        };
        let mut content = div().flex().flex_col().gap_5().w_full().min_w_0();
        let mode = settings.authentication.mode;
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
                            let view = cx.entity();
                            segmented(
                                "authentication",
                                &[
                                    t!("auth.pairing").to_string(),
                                    t!("auth.password").to_string(),
                                    t!("auth.otp").to_string(),
                                    t!("auth.none").to_string(),
                                ],
                                match mode {
                                    removent_core::AuthenticationMode::PairingCode => 0,
                                    removent_core::AuthenticationMode::Password => 1,
                                    removent_core::AuthenticationMode::Otp => 2,
                                    removent_core::AuthenticationMode::None => 3,
                                },
                                Rc::new(move |i, _, _, app| {
                                    view.update(app, |this, cx| {
                                        let mode = match i {
                                            1 => removent_core::AuthenticationMode::Password,
                                            2 => removent_core::AuthenticationMode::Otp,
                                            3 => removent_core::AuthenticationMode::None,
                                            _ => removent_core::AuthenticationMode::PairingCode,
                                        };
                                        this.save_authentication(mode, cx)
                                    })
                                }),
                                cx,
                            )
                        })
                        .child(setting_label(
                            t!("auth.password").to_string(),
                            t!("auth.password_hint").to_string(),
                            cx,
                        ))
                        .child(
                            div()
                                .flex()
                                .gap_3()
                                .items_center()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .child(form_input(&self.auth_password_input).mask_toggle()),
                                )
                                .child(
                                    Button::new("save-auth-password")
                                        .label(t!("auth.use_password").to_string())
                                        .primary()
                                        .disabled(
                                            self.auth_password_input.read(cx).value().is_empty(),
                                        )
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.save_authentication(
                                                removent_core::AuthenticationMode::Password,
                                                cx,
                                            )
                                        })),
                                ),
                        )
                        .when(mode == removent_core::AuthenticationMode::Otp, |el| {
                            el.child(setting_label(
                                t!("auth.otp_setup").to_string(),
                                t!("auth.otp_setup_hint").to_string(),
                                cx,
                            ))
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .child(settings.authentication.otp_secret.clone()),
                            )
                            .child(
                                Button::new("copy-otp-secret")
                                    .label(t!("auth.copy_secret").to_string())
                                    .outline()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                            this.engine.settings().authentication.otp_secret,
                                        ));
                                    })),
                            )
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
        if mode == removent_core::AuthenticationMode::PairingCode {
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
