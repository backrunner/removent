use super::*;

impl HomeView {
    pub(super) fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let tone = match self.status_tone {
            StatusTone::Err => colors.danger,
            StatusTone::Warn => colors.warning,
            StatusTone::Ok => colors.success,
            StatusTone::Info => colors.muted_foreground,
        };
        div()
            .min_h(px(32.))
            .flex_shrink_0()
            .px_4()
            .py_2()
            .flex()
            .items_center()
            .gap_2()
            .bg(colors.sidebar)
            .border_t_1()
            .border_color(colors.border)
            .when_some(self.save_warning.clone(), |el, warning| {
                el.child(
                    div()
                        .text_size(px(11.))
                        .text_color(colors.warning)
                        .child(warning),
                )
            })
            .child(dot(tone))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(11.))
                    .text_color(tone)
                    .child(self.status.clone()),
            )
    }

    pub(super) fn render_pin_dialog(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let dialog = self.pin_dialog.as_ref()?;
        let colors = cx.theme().colors;
        let card = div()
            .w(px(360.))
            .p_6()
            .rounded(px(16.))
            .bg(colors.popover)
            .border_1()
            .border_color(colors.border)
            .shadow(theme::popup_shadow(cx.theme().is_dark()))
            .flex()
            .flex_col()
            .gap_4();
        let card = match dialog {
            PinDialog::Display(pin) => card
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(t!("pairing.request_title").to_string()),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.muted_foreground)
                        .child(
                            t!(if pin.len() == 12 {
                                "pairing.invitation_hint"
                            } else {
                                "pairing.display_desc"
                            })
                            .to_string(),
                        ),
                )
                .child(
                    div().flex().justify_center().py_2().child(
                        div()
                            .text_size(px(32.))
                            .font_family(cx.theme().mono_font_family.clone())
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(colors.foreground)
                            .child(group_pin(pin)),
                    ),
                )
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("copy-pairing-code")
                                .label(t!("pairing.copy").to_string())
                                .outline()
                                .on_click({
                                    let pin = pin.clone();
                                    move |_, _, cx| {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                            pin.clone(),
                                        ))
                                    }
                                }),
                        )
                        .when(pin.len() == 12, |el| {
                            el.child(
                                Button::new("revoke-pairing-code")
                                    .label(t!("pairing.revoke").to_string())
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.engine.revoke_pairing_code();
                                        this.pin_dialog = None;
                                        cx.notify();
                                    })),
                            )
                        })
                        .child(
                            Button::new("dismiss-pin")
                                .label(t!("action.close").to_string())
                                .outline()
                                .on_click(cx.listener(|this, _, _w, cx| {
                                    this.pin_dialog = None;
                                    cx.notify();
                                })),
                        ),
                ),
            PinDialog::Trust {
                destination, relay, ..
            } => card
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(
                            t!(if *relay {
                                "trust.relay_title"
                            } else {
                                "trust.computer_title"
                            })
                            .to_string(),
                        ),
                )
                .child(div().text_size(px(13.)).child(destination.clone()))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.muted_foreground)
                        .child(t!("trust.description").to_string()),
                )
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            div().debug_selector(|| "cancel-certificate".into()).child(
                                Button::new("cancel-certificate")
                                    .label(t!("action.cancel").to_string())
                                    .ghost()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.cancel_pin(window, cx)
                                    })),
                            ),
                        )
                        .child(
                            div().debug_selector(|| "trust-certificate".into()).child(
                                Button::new("trust-certificate")
                                    .label(t!("trust.connect").to_string())
                                    .primary()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.confirm_certificate(window, cx)
                                    })),
                            ),
                        ),
                ),
            PinDialog::Entry(_) => card
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(
                            t!(match self.auth_mode {
                                removent_core::AuthenticationMode::Password => "auth.password",
                                removent_core::AuthenticationMode::Otp => "auth.otp",
                                _ => "pairing.entry_title",
                            })
                            .to_string(),
                        ),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.muted_foreground)
                        .child(
                            t!(match self.auth_mode {
                                removent_core::AuthenticationMode::Password => "auth.password_desc",
                                removent_core::AuthenticationMode::Otp => "auth.otp_desc",
                                _ => "pairing.entry_desc",
                            })
                            .to_string(),
                        ),
                )
                .child(form_input(&self.pin_input))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("cancel-pin")
                                .label(t!("action.cancel").to_string())
                                .ghost()
                                .on_click(cx.listener(|this, _, w, cx| this.cancel_pin(w, cx))),
                        )
                        .child(
                            Button::new("submit-pin")
                                .disabled({
                                    let pin = self.pin_input.read(cx).value();
                                    !self.auth_mode.valid_input(&pin)
                                })
                                .label(t!("auth.connect").to_string())
                                .primary()
                                .on_click(cx.listener(|this, _, w, cx| this.submit_pin(w, cx))),
                        ),
                ),
        };
        Some(modal_overlay("pin-overlay", self.dialog_seq, card, cx))
    }

    pub(super) fn render_admission_dialog(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let a = self.admission.as_ref()?;
        let colors = cx.theme().colors;
        let card = div()
            .w(px(400.))
            .p_6()
            .rounded(px(16.))
            .bg(colors.popover)
            .border_1()
            .border_color(colors.border)
            .shadow(theme::popup_shadow(cx.theme().is_dark()))
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(t!("admission.title").to_string()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(device_glyph(32., &colors))
                    .child(
                        div()
                            .child(div().text_size(px(13.)).child(a.peer_name.clone()))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_color(colors.muted_foreground)
                                    .child(format!("fp:{}", a.peer_fp_short)),
                            ),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(colors.muted_foreground)
                    .child(t!("admission.desc").to_string()),
            )
            .child(
                // Live countdown bar: drains over the daemon's 30s admission window
                // (mirrors removent-daemon DEFAULT_ADMISSION_TIMEOUT, protocol §4.4).
                div().h(px(3.)).rounded_full().bg(colors.border).child(
                    div()
                        .h_full()
                        .rounded_full()
                        .bg(colors.accent.opacity(0.6))
                        .with_animation(
                            ElementId::NamedInteger("admission-countdown".into(), a.request_id),
                            Animation::new(Duration::from_secs(30)),
                            |this, delta| this.w(relative(1. - delta)),
                        ),
                ),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("deny")
                            .label(t!("action.deny").to_string())
                            .outline()
                            .on_click(
                                cx.listener(|this, _, _w, cx| this.answer_admission(false, cx)),
                            ),
                    )
                    .child(
                        Button::new("allow")
                            .label(t!("action.allow").to_string())
                            .primary()
                            .on_click(
                                cx.listener(|this, _, _w, cx| this.answer_admission(true, cx)),
                            ),
                    ),
            );
        Some(modal_overlay("admission-overlay", a.request_id, card, cx))
    }
}
