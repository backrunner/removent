use super::*;

impl Render for HomeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let recovery = self.engine.client_recovery().state();
        let offline_overlay = self.connecting.is_some()
            && matches!(
                recovery,
                removent_client::recovery::State::Offline
                    | removent_client::recovery::State::Reconnecting {
                        after_offline: true
                    }
            );
        let connection_visible =
            self.pin_dialog.is_none() && self.admission.is_none() && !offline_overlay;
        if let Some(dialog) = &self.connection_dialog {
            dialog.update(cx, |form, cx| {
                form.set_obscured(!connection_visible, &self.focus, window, cx)
            });
        }
        let colors = cx.theme().colors;
        let detail = if self.settings_open {
            self.render_settings(cx).into_any_element()
        } else {
            match self.selected.clone() {
                Some(Selection::Device(fp)) => match self.devices.get(&fp).cloned() {
                    Some(row) => self.render_device_detail(&fp, &row, cx).into_any_element(),
                    None => self.render_empty_detail(cx).into_any_element(),
                },
                Some(Selection::Saved(id)) => {
                    match self.saved.iter().find(|s| s.id == id).cloned() {
                        Some(entry) => self.render_saved_detail(&entry, cx).into_any_element(),
                        None => self.render_empty_detail(cx).into_any_element(),
                    }
                }
                None => self.render_empty_detail(cx).into_any_element(),
            }
        };

        div()
            .key_context("Home")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &HomeSettings, window, cx| {
                if this.pin_dialog.is_none()
                    && this.admission.is_none()
                    && this.connection_dialog.is_none()
                {
                    this.reset_authentication_form(window, cx);
                    this.settings_open = !this.settings_open;
                    window.focus(&this.focus, cx);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &HomeConnect, window, cx| {
                if this.pin_dialog.is_none()
                    && this.admission.is_none()
                    && this.connecting.is_none()
                {
                    this.open_connection_dialog(None, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &HomeSearch, window, cx| {
                if this.pin_dialog.is_none()
                    && this.admission.is_none()
                    && this.connection_dialog.is_none()
                {
                    this.search_input
                        .update(cx, |input, cx| input.focus(window, cx));
                    cx.notify();
                }
            }))
            // Esc closes the topmost dialog first, matching the render order
            // below (admission renders after the PIN dialog, hence sits on
            // top): admission = deny (the safe default for a connection
            // request), Entry PIN = cancel pairing, Display PIN = dismiss.
            .on_action(cx.listener(|this, _: &HomeEscape, window, cx| {
                if this.admission.is_some() {
                    this.answer_admission(false, cx);
                } else if matches!(
                    this.pin_dialog,
                    Some(PinDialog::Entry(_) | PinDialog::Trust { .. })
                ) {
                    this.cancel_pin(window, cx);
                } else if matches!(this.pin_dialog, Some(PinDialog::Display(_))) {
                    this.pin_dialog = None;
                    window.focus(&this.focus, cx);
                    cx.notify();
                } else if let Some(dialog) = &this.connection_dialog {
                    dialog.update(cx, |form, cx| form.back(window, cx));
                } else if this.settings_open {
                    this.reset_authentication_form(window, cx);
                    this.settings_open = false;
                    window.focus(&this.focus, cx);
                    cx.notify();
                }
            }))
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().transparent)
            .text_color(colors.foreground)
            .child(self.render_title_bar(cx))
            .when(
                self.connecting.is_some() && self.connection_dialog.is_none(),
                |el| {
                    el.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .px_5()
                            .py_2()
                            .flex_shrink_0()
                            .bg(colors.accent.opacity(0.08))
                            .border_b_1()
                            .border_color(colors.border)
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .child(Spinner::new().icon(icon_16("loader-circle")).small()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .text_size(px(13.))
                                            .whitespace_nowrap()
                                            .child(crate::ui::connection::stage_label(
                                                self.connection_stage,
                                            )),
                                    )
                                    .child(
                                        div()
                                            .min_w_0()
                                            .text_size(px(12.))
                                            .text_color(colors.muted_foreground)
                                            .truncate()
                                            .child(self.connecting.clone().unwrap_or_default()),
                                    ),
                            )
                            .child(
                                Button::new("cancel-pending-session")
                                    .small()
                                    .outline()
                                    .flex_shrink_0()
                                    .label(t!("connection.cancel_attempt").to_string())
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.cancel_connection(cx)),
                                    ),
                            ),
                    )
                },
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .overflow_hidden()
                    .child(self.render_sidebar(window, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .bg(colors.background)
                            .overflow_hidden()
                            .child(detail),
                    ),
            )
            .child(self.render_status_bar(cx))
            .children(
                self.connection_dialog
                    .as_ref()
                    .filter(|_| connection_visible)
                    .map(|dialog| {
                        modal_overlay(
                            "connection-overlay",
                            self.dialog_seq,
                            div()
                                .w(px(480.))
                                .max_w(window.viewport_size().width - px(48.))
                                .rounded(px(20.))
                                .bg(colors.popover)
                                .border_1()
                                .border_color(colors.border)
                                .shadow(theme::popup_shadow(cx.theme().is_dark()))
                                .child(dialog.clone()),
                            cx,
                        )
                    }),
            )
            .children(self.render_pin_dialog(cx))
            .children(self.render_admission_dialog(cx))
            .when(offline_overlay, |el| {
                let offline = recovery == removent_client::recovery::State::Offline;
                el.child(modal_overlay(
                    "network-overlay",
                    self.dialog_seq,
                    div()
                        .w(px(360.))
                        .p_5()
                        .rounded(px(18.))
                        .bg(colors.popover)
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_3()
                        .child(
                            t!(if offline {
                                "viewer.network_offline"
                            } else {
                                "viewer.network_reconnecting"
                            })
                            .to_string(),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .child(t!("viewer.network_retry_hint").to_string()),
                        )
                        .child(
                            Button::new("cancel-offline-connection")
                                .label(t!("connection.cancel_attempt").to_string())
                                .on_click(cx.listener(|this, _, _, cx| this.cancel_connection(cx))),
                        ),
                    cx,
                ))
            })
    }
}
