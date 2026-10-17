use super::*;
use gpui_component::{
    scroll::ScrollableElement,
    text::{TextView, TextViewStyle},
};

/// A bounded reading surface keeps lengthy release notes from taking over settings.
fn release_notes(
    version: &str,
    notes: &str,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let colors = cx.theme().colors;
    // A different release starts at the top of its own notes.
    let id = ElementId::Name(format!("release-notes-{version}").into());
    let scroll = window
        .use_keyed_state(id.clone(), cx, |_, _| gpui::ScrollHandle::default())
        .read(cx)
        .clone();
    let style = TextViewStyle::default()
        .paragraph_gap(gpui::rems(0.75))
        .heading_font_size(|level, _| px(if level <= 2 { 15. } else { 13. }))
        .inline_code(gpui::HighlightStyle {
            color: Some(colors.foreground),
            background_color: Some(colors.secondary_hover),
            ..Default::default()
        });

    div()
        .debug_selector(|| "update-release-notes".into())
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_size(px(12.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(colors.muted_foreground)
                .child(t!("update.release_notes").to_string()),
        )
        .child(
            div()
                .id(id)
                .debug_selector(|| "update-release-notes-viewport".into())
                .w_full()
                .min_w_0()
                .max_h(px(260.))
                .rounded(px(8.))
                .border_1()
                .border_color(colors.border)
                .bg(colors.background)
                .relative()
                .overflow_y_scroll()
                .track_scroll(&scroll)
                .vertical_scrollbar(&scroll)
                // GPUI bubbles wheel events to every scrollable ancestor.
                // Reading notes must not move the update action out of view.
                .on_scroll_wheel(move |_, _, cx| {
                    if scroll.max_offset().y > px(0.) {
                        cx.stop_propagation();
                    }
                })
                .child(
                    div()
                        .debug_selector(|| "update-release-notes-content".into())
                        .p_3()
                        .child(
                            TextView::markdown("release-notes-markdown", notes.trim().to_owned())
                                .style(style)
                                .selectable(true)
                                .text_size(px(13.))
                                .line_height(relative(1.55))
                                .text_color(colors.foreground)
                                .on_link_click(|url, _, _, cx| {
                                    if url.starts_with("https://") || url.starts_with("http://") {
                                        cx.open_url(url);
                                    }
                                }),
                        ),
                ),
        )
}

impl HomeView {
    /// Software update (release.md §3): current version, check button, auto-check
    /// toggle, and the state-machine line (available/downloading/ready/failed).
    pub(super) fn render_update_section(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors;
        let settings = self.engine.settings();
        let auto_check = settings.update_check_enabled;
        let busy = self.update_status.is_busy();
        let last_check = self
            .engine
            .last_update_check()
            .and_then(|last| super::timestamps::local_clock_time(last, &chrono::Local));

        // One status line per state, colored by severity; spinners while working.
        let status_line = |text: String, color: gpui::Hsla| {
            div().text_size(px(12.)).text_color(color).child(text)
        };
        let spinner_line = |text: String| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    Spinner::new()
                        .icon(icon_16("loader-circle"))
                        .color(colors.muted_foreground),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.muted_foreground)
                        .child(text),
                )
        };

        let mut state_block = div().flex().flex_col().gap_3().px_4().pb_3();
        match &self.update_status {
            UpdateStatus::Idle | UpdateStatus::UpToDate { manual: false } => {}
            UpdateStatus::Checking => {
                state_block = state_block.child(spinner_line(t!("update.checking").to_string()));
            }
            UpdateStatus::UpToDate { manual: true } => {
                state_block = state_block.child(
                    status_line(t!("update.up_to_date").to_string(), colors.success)
                        .debug_selector(|| "update-up-to-date".into()),
                );
            }
            UpdateStatus::Available { version, notes } => {
                state_block = state_block.when(!notes.trim().is_empty(), |el| {
                    el.child(release_notes(version, notes, window, cx))
                });
            }
            UpdateStatus::Downloading { version } => {
                state_block = state_block.child(spinner_line(
                    t!("update.downloading", version = version).to_string(),
                ));
            }
            UpdateStatus::Verifying { .. } => {
                state_block = state_block.child(spinner_line(t!("update.verifying").to_string()));
            }
            UpdateStatus::ReadyToInstall { version } => {
                let session_active = self.engine.session_active();
                state_block = state_block.child(status_line(
                    t!("update.ready", version = version).to_string(),
                    colors.accent,
                ));
                if session_active {
                    // Deferred: installing would kill the live session.
                    state_block = state_block.child(status_line(
                        t!("update.session_active_hint").to_string(),
                        colors.warning,
                    ));
                }
                state_block = state_block.child(
                    div().flex().justify_end().child(
                        Button::new("update-install")
                            .label(t!("update.install_now").to_string())
                            .primary()
                            .compact()
                            .on_click(cx.listener(|this, _, _w, cx| {
                                // The engine re-checks for live sessions; surface a refusal.
                                if let Err(reason) = this.engine.install_update() {
                                    this.update_status = UpdateStatus::Failed(reason);
                                }
                                cx.notify();
                            })),
                    ),
                );
            }
            UpdateStatus::Swapping { .. } | UpdateStatus::Relaunching => {
                state_block = state_block.child(spinner_line(t!("update.installing").to_string()));
            }
            UpdateStatus::Failed(reason) => {
                state_block = state_block.child(status_line(reason.clone(), colors.danger));
            }
        }

        let available = matches!(self.update_status, UpdateStatus::Available { .. });
        let current_version = || {
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .text_size(px(if available { 12. } else { 13. }))
                .text_color(if available {
                    colors.muted_foreground
                } else {
                    colors.foreground
                })
                .child(t!("update.current").to_string())
                .child(
                    div()
                        .text_color(colors.muted_foreground)
                        .child(env!("CARGO_PKG_VERSION")),
                )
        };
        let last_check_line = || {
            div()
                .debug_selector(|| "update-last-check".into())
                .text_size(px(11.))
                .text_color(colors.muted_foreground)
                .child(match &last_check {
                    Some(time) => t!("update.last_checked", time = time).to_string(),
                    None => t!("update.never_checked").to_string(),
                })
        };
        let check_button = |cx: &mut Context<Self>| {
            Button::new("update-check")
                .label(t!("update.check_now").to_string())
                .compact()
                .disabled(busy)
                .on_click(cx.listener(|this, _, _w, cx| {
                    this.engine.check_for_updates();
                    cx.notify();
                }))
        };
        let summary = div().flex().items_center().gap_4().px_4().py_4();
        let summary = match &self.update_status {
            UpdateStatus::Available { version, .. } => summary
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(17.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(t!("update.available", version = version).to_string()),
                        )
                        .child(current_version()),
                )
                .child(
                    div()
                        .debug_selector(|| "update-download".into())
                        .flex_shrink_0()
                        .child(
                            Button::new("update-download")
                                .label(t!("update.download_install").to_string())
                                .primary()
                                .h(px(30.))
                                .on_click(cx.listener(|this, _, _w, cx| {
                                    this.engine.download_update();
                                    cx.notify();
                                })),
                        ),
                ),
            _ => summary
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(current_version())
                        .child(last_check_line()),
                )
                .child(check_button(cx).outline()),
        };

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(group_title(t!("update.section").to_string(), cx))
            .child(
                form_group(cx)
                    .child(summary)
                    .when(
                        !matches!(
                            self.update_status,
                            UpdateStatus::Idle | UpdateStatus::UpToDate { manual: false }
                        ),
                        |el| el.child(state_block),
                    )
                    .when(available, |el| {
                        el.child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_3()
                                .px_4()
                                .pb_3()
                                .child(last_check_line())
                                .child(check_button(cx).ghost()),
                        )
                    })
                    .child(Divider::horizontal())
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .px_4()
                            .py_3()
                            .child(setting_label(
                                t!("update.channel").to_string(),
                                t!("update.channel_hint").to_string(),
                                cx,
                            ))
                            .child({
                                let view = cx.entity();
                                crate::ui::widgets::segmented_with_disabled(
                                    "update-channel",
                                    &[
                                        t!("update.channel_stable").to_string(),
                                        t!("update.channel_beta").to_string(),
                                    ],
                                    usize::from(settings.update_channel == UpdateChannel::Beta),
                                    busy,
                                    Rc::new(move |i, _, _, app| {
                                        view.update(app, |this, cx| {
                                            let channel = if i == 1 {
                                                UpdateChannel::Beta
                                            } else {
                                                UpdateChannel::Stable
                                            };
                                            match this.engine.set_update_channel(channel) {
                                                Ok(()) => {
                                                    this.update_status =
                                                        this.engine.update_status();
                                                    this.engine.check_for_updates_silently();
                                                }
                                                Err(reason) => {
                                                    this.set_status(reason, StatusTone::Err)
                                                }
                                            }
                                            cx.notify();
                                        });
                                    }),
                                    cx,
                                )
                            }),
                    )
                    .child(Divider::horizontal())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .px_4()
                            .py_3()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .child(t!("update.auto_check").to_string()),
                            )
                            .child(
                                Switch::new("update-auto-check")
                                    .small()
                                    .checked(auto_check)
                                    .on_click(cx.listener(|this, _checked, _w, cx| {
                                        let on = !this.engine.settings().update_check_enabled;
                                        this.persist_settings(|s| s.update_check_enabled = on);
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
    }
}
