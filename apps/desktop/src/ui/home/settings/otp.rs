//! Local authenticator enrollment; QR and plaintext secrets are revealed on demand.

use super::*;

impl HomeView {
    fn toggle_otp_qr(&mut self, cx: &mut Context<Self>) {
        if self.otp_qr.take().is_none() {
            let settings = self.engine.settings();
            let name: String = settings.device_name.chars().take(48).collect();
            let account = format!("{name} ({})", self.my_fp_short);
            self.otp_qr = settings
                .authentication
                .otp_provisioning_uri(&account)
                .ok()
                .and_then(|uri| qrcode::QrCode::new(uri.as_bytes()).ok());
            self.otp_setup_error = self.otp_qr.is_none();
        } else {
            self.otp_setup_error = false;
        }
        cx.notify();
    }

    pub(super) fn render_otp_setup(&self, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors;
        let qr_open = self.otp_qr.is_some();
        let qr_toggle = Button::new("toggle-otp-qr")
            .label(
                t!(if qr_open {
                    "auth.otp_collapse"
                } else {
                    "auth.otp_expand"
                })
                .to_string(),
            )
            .icon(icon_16(if qr_open {
                "chevron-up"
            } else {
                "chevron-down"
            }))
            .tooltip(
                t!(if qr_open {
                    "auth.otp_hide_qr"
                } else {
                    "auth.otp_show_qr"
                })
                .to_string(),
            )
            .ghost()
            .small()
            .w(px(72.))
            .h(px(28.))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_otp_qr(cx)));
        let qr_toggle = div()
            .flex_shrink_0()
            .debug_selector(|| "otp-toggle-qr".into())
            .child(qr_toggle);
        let secret_toggle = Button::new("toggle-otp-secret")
            .icon(icon_16(if self.otp_secret_visible {
                "eye-off"
            } else {
                "eye"
            }))
            .tooltip(
                t!(if self.otp_secret_visible {
                    "auth.otp_hide_secret"
                } else {
                    "auth.otp_show_secret"
                })
                .to_string(),
            )
            .ghost()
            .small()
            .size(px(28.))
            .on_click(cx.listener(|this, _, _, cx| {
                this.otp_secret_visible = !this.otp_secret_visible;
                cx.notify();
            }));
        let secret_toggle = div()
            .flex_shrink_0()
            .debug_selector(|| "otp-toggle-secret".into())
            .child(secret_toggle);
        let secret = div()
            .flex_1()
            .min_w_0()
            .text_size(px(12.))
            .text_ellipsis()
            .when(self.otp_secret_visible, |el| {
                el.font_family(cx.theme().mono_font_family.clone())
                    .debug_selector(|| "otp-secret-visible".into())
            })
            .text_color(if self.otp_secret_visible {
                colors.foreground
            } else {
                colors.muted_foreground
            })
            .child(if self.otp_secret_visible {
                self.engine.settings().authentication.otp_secret
            } else {
                t!("auth.otp_secret_hidden").to_string()
            });
        let content = form_group(cx)
            .child(div().p_4().child(setting_label(
                t!("auth.otp_setup").to_string(),
                t!("auth.otp_setup_hint").to_string(),
                cx,
            )))
            .child(Divider::horizontal())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap_3()
                            .child(div().flex_1().min_w_0().child(setting_label(
                                t!("auth.otp_qr_label").to_string(),
                                t!("auth.otp_scan_hint").to_string(),
                                cx,
                            )))
                            .child(qr_toggle),
                    )
                    .when_some(self.otp_qr.clone(), |el, code| {
                        el.child(
                            div()
                                .debug_selector(|| "otp-qr-visible".into())
                                .flex()
                                .items_start()
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .p_2()
                                        .rounded(px(8.))
                                        .bg(gpui::rgb(0xffffff))
                                        .border_1()
                                        .border_color(colors.border)
                                        .child(qr_canvas(code)),
                                ),
                        )
                    })
                    .when(self.otp_setup_error, |el| {
                        el.child(
                            div()
                                .text_size(px(12.))
                                .text_color(colors.danger)
                                .child(t!("auth.otp_setup_failed").to_string()),
                        )
                    }),
            )
            .child(Divider::horizontal())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .child(setting_label(
                        t!("auth.otp_manual_label").to_string(),
                        t!("auth.otp_manual_hint").to_string(),
                        cx,
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .h(px(40.))
                            .pl_3()
                            .pr_1()
                            .rounded(px(8.))
                            .border_1()
                            .border_color(colors.border)
                            .bg(colors.muted.opacity(0.5))
                            .child(secret)
                            .child(secret_toggle)
                            .child(
                                Button::new("copy-otp-secret")
                                    .icon(icon_16("copy"))
                                    .tooltip(t!("auth.copy_secret").to_string())
                                    .ghost()
                                    .small()
                                    .size(px(28.))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                            this.engine.settings().authentication.otp_secret,
                                        ));
                                    })),
                            ),
                    ),
            );
        #[cfg(test)]
        let content = content.debug_selector(|| "otp-setup".into());
        content
    }
}

fn qr_canvas(code: qrcode::QrCode) -> impl IntoElement {
    // An integer module size and four-module quiet zone keep the code crisp in
    // both themes. Nothing is written to disk or sent to a QR generation service.
    let module = (208. / (code.width() + 8) as f32).floor().max(1.);
    let side = module * (code.width() + 8) as f32;
    gpui::canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            window.paint_quad(gpui::fill(bounds, gpui::rgb(0xffffff)));
            for y in 0..code.width() {
                for x in 0..code.width() {
                    if code[(x, y)] == qrcode::Color::Dark {
                        let origin = bounds.origin
                            + gpui::point(px((x + 4) as f32 * module), px((y + 4) as f32 * module));
                        window.paint_quad(gpui::fill(
                            gpui::Bounds::new(origin, gpui::size(px(module), px(module))),
                            gpui::rgb(0),
                        ));
                    }
                }
            }
        },
    )
    .size(px(side))
    .flex_shrink_0()
}
