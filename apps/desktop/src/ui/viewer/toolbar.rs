use super::*;

impl ViewerView {
    pub(super) fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        div()
            .absolute()
            .top(px(42.))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .when(self.info_visible, |el| el.justify_end().pr_4())
            .child(
                overlay_chip(cx)
                    .debug_selector(|| "viewer-toolbar-chip".into())
                    .child(
                        Button::new("toggle-info")
                            .icon(icon_16("gauge"))
                            .segment(self.info_visible)
                            .compact()
                            .tooltip(t!("viewer.info_tooltip").to_string())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_info(window, cx);
                            })),
                    )
                    .child(
                        Button::new("toggle-fullscreen")
                            .icon(icon_16("maximize"))
                            .ghost()
                            .compact()
                            .tooltip(t!("viewer.fullscreen_tooltip").to_string())
                            .on_click(cx.listener(|_this, _, window, cx| {
                                window.toggle_fullscreen();
                                cx.notify();
                            })),
                    )
                    .child(div().w(px(1.)).h(px(16.)).mx_1().bg(colors.border))
                    .child(
                        Button::new("disconnect")
                            .icon(icon_16("power"))
                            .destructive()
                            .compact()
                            .tooltip(t!("viewer.disconnect_tooltip").to_string())
                            .on_click(cx.listener(|this, _, window, _cx| {
                                this.disconnect(window);
                            })),
                    ),
            )
            // Entry transition: fade in while settling 8px down into place.
            .with_animation(
                ElementId::NamedInteger("viewer-toolbar".into(), self.toolbar_seq),
                motion::toolbar_enter(),
                |this, delta| this.top(px(34.) + delta * px(8.)).opacity(delta),
            )
    }

    pub(super) fn render_info(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mono = cx.theme().mono_font_family.clone();
        let muted = gpui::white().opacity(0.65);
        let mut rows = vec![
            (t!("viewer.info_peer").to_string(), self.peer_name.clone()),
            (
                t!("viewer.info_protocol").to_string(),
                self.diagnostics.codec.clone(),
            ),
            (
                t!("viewer.info_duration").to_string(),
                format_duration(self.started.elapsed()),
            ),
            (
                t!("viewer.info_resolution").to_string(),
                if self.width > 0 {
                    format!("{} × {}", self.width, self.height)
                } else {
                    "—".into()
                },
            ),
            (
                t!("viewer.info_display_fps").to_string(),
                format!("{:.1} fps", self.fps_shown),
            ),
            (
                t!("viewer.info_frame_age").to_string(),
                self.last_frame
                    .map(|last| format!("{:.1} s", last.elapsed().as_secs_f64()))
                    .unwrap_or_else(|| "—".into()),
            ),
        ];
        if let Some(stats) = self.diagnostics.vnc {
            rows.extend([
                (
                    t!("viewer.info_receive_fps").to_string(),
                    format!("{:.1} fps", self.receive_fps),
                ),
                (
                    t!("viewer.info_receive_rate").to_string(),
                    format!("{:.2} MB/s", self.receive_rate / 1_000_000.),
                ),
                (
                    t!("viewer.info_received").to_string(),
                    format!("{:.1} MB", stats.received_bytes as f64 / 1_000_000.),
                ),
                (
                    t!("viewer.info_decode").to_string(),
                    if stats.received_frames > 0 {
                        format!("{:.1} ms", stats.decode_us as f64 / 1000.)
                    } else {
                        "—".into()
                    },
                ),
                (
                    t!("viewer.info_update").to_string(),
                    if stats.received_frames > 0 {
                        format!("{:.1} ms", stats.update_us as f64 / 1000.)
                    } else {
                        "—".into()
                    },
                ),
            ]);
        }
        if let Some(input) = self.diagnostics.input {
            rows.extend([
                (
                    t!("viewer.info_input_pending").to_string(),
                    input.pending.to_string(),
                ),
                (
                    t!("viewer.info_input_age").to_string(),
                    format_ms(input.oldest_pending),
                ),
                (
                    t!("viewer.info_input_delay").to_string(),
                    format_ms(input.dispatch_delay),
                ),
                (
                    t!("viewer.info_input_sent").to_string(),
                    input.sent.to_string(),
                ),
                (
                    t!("viewer.info_coalesced").to_string(),
                    input.coalesced.to_string(),
                ),
            ]);
        }
        let max_height =
            (f32::from(window.viewport_size().height - TITLE_BAR_HEIGHT) - 32.).max(100.);
        div()
            .id("viewer-info")
            .debug_selector(|| "viewer-info".into())
            .absolute()
            .top(px(16.))
            .left(px(16.))
            .w(px(280.))
            .max_h(px(max_height))
            .flex()
            .flex_col()
            .rounded(px(12.))
            .bg(gpui::rgba(0x101418D1))
            .border_1()
            .border_color(gpui::white().opacity(0.14))
            .text_color(gpui::white().opacity(0.92))
            // Panel interaction is local. Mouse-up still bubbles so a remote
            // drag ending on the panel releases its held button.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
            .on_mouse_move(cx.listener(|this, _, _, cx| {
                if this.buttons == 0 {
                    cx.stop_propagation();
                }
            }))
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .flex_shrink_0()
                    .child(icon_16("gauge").text_color(muted))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.))
                            .child(t!("viewer.info_title").to_string()),
                    )
                    .child(
                        div()
                            .id("close-viewer-info")
                            .debug_selector(|| "close-viewer-info".into())
                            .cursor_pointer()
                            .p_1()
                            .rounded(px(4.))
                            .hover(|el| el.bg(gpui::white().opacity(0.12)))
                            .child(icon_16("x").text_color(muted))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_info(window, cx);
                                cx.stop_propagation();
                            })),
                    ),
            )
            .child(
                div()
                    .id("viewer-info-rows")
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.info_scroll)
                    .px_3()
                    .pb_3()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(muted)
                            .mb_2()
                            .child(t!("viewer.info_idle_hint").to_string()),
                    )
                    .children(rows.into_iter().map(|(label, value)| {
                        div()
                            .flex()
                            .items_start()
                            .gap_3()
                            .py_1()
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(11.))
                                    .text_color(muted)
                                    .child(label),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_right()
                                    .text_size(px(11.))
                                    .font_family(mono.clone())
                                    .child(value),
                            )
                    }))
                    .when(self.diagnostics.input.is_some(), |el| {
                        el.child(
                            div()
                                .text_size(px(11.))
                                .text_color(muted)
                                .mt_2()
                                .child(t!("viewer.info_input_hint").to_string()),
                        )
                    }),
            )
    }

    pub(super) fn render_ended_overlay(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        div()
            .absolute()
            .inset_0()
            .bg(crate::theme::scrim(cx.theme().is_dark()))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(320.))
                    .p_5()
                    .rounded(px(18.))
                    .bg(colors.popover)
                    .border_1()
                    .border_color(colors.border)
                    .shadow(crate::theme::popup_shadow(cx.theme().is_dark()))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(t!("viewer.ended_title").to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(colors.muted_foreground)
                            .child(t!("viewer.ended_desc").to_string()),
                    )
                    .child(
                        Button::new("close-viewer")
                            .label(t!("viewer.close_window").to_string())
                            .primary()
                            .on_click(cx.listener(|_this, _, window, _cx| {
                                window.remove_window();
                            })),
                    ),
            )
            // Entry transition: the whole overlay fades in.
            .with_animation(
                "viewer-ended-fade",
                motion::overlay_fade(),
                |this, delta| this.opacity(delta),
            )
    }
}
