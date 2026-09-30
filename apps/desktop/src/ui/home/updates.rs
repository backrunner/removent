use super::*;

impl HomeView {
    /// Software update (release.md §3): current version, check button, auto-check
    /// toggle, and the state-machine line (available/downloading/ready/failed).
    pub(super) fn render_update_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let settings = self.engine.settings();
        let auto_check = settings.update_check_enabled;
        let busy = self.update_status.is_busy();

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

        let mut state_block = div().flex().flex_col().gap_2().px_4().pb_3();
        match &self.update_status {
            UpdateStatus::Idle => {}
            UpdateStatus::Checking => {
                state_block = state_block.child(spinner_line(t!("update.checking").to_string()));
            }
            UpdateStatus::UpToDate => {
                state_block = state_block.child(status_line(
                    t!("update.up_to_date").to_string(),
                    colors.success,
                ));
            }
            UpdateStatus::Available { version, notes } => {
                state_block = state_block
                    .child(status_line(
                        t!("update.available", version = version).to_string(),
                        colors.accent,
                    ))
                    .when(!notes.is_empty(), |el| {
                        el.child(
                            div()
                                .text_size(px(11.))
                                .text_color(colors.muted_foreground)
                                .child(notes.clone()),
                        )
                    })
                    .child(
                        div().flex().justify_end().child(
                            Button::new("update-download")
                                .label(t!("update.download_install").to_string())
                                .primary()
                                .compact()
                                .on_click(cx.listener(|this, _, _w, cx| {
                                    this.engine.download_update();
                                    cx.notify();
                                })),
                        ),
                    );
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

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(group_title(t!("update.section").to_string(), cx))
            .child(
                form_group(cx)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .px_4()
                            .py_3()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .child(t!("update.current").to_string()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(12.))
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_color(colors.muted_foreground)
                                    .child(env!("CARGO_PKG_VERSION")),
                            )
                            .child(
                                Button::new("update-check")
                                    .label(t!("update.check_now").to_string())
                                    .outline()
                                    .compact()
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.engine.check_for_updates();
                                        cx.notify();
                                    })),
                            ),
                    )
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
                                                    this.engine.check_for_updates();
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
                    )
                    .child(state_block),
            )
    }
}
