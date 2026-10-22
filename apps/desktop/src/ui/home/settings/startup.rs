use super::*;

impl HomeView {
    pub(super) fn render_startup_settings(&self, cx: &mut Context<Self>) -> Option<Div> {
        #[cfg(not(target_os = "macos"))]
        {
            let _ = cx;
            None
        }
        #[cfg(target_os = "macos")]
        {
            let state = self.engine.startup_settings();
            let colors = cx.theme().colors;
            let external = self.engine.startup_is_managed_externally();
            let disabled = state.busy || external;
            let system = state.system.as_ref();
            let installed = system.is_some_and(|s| s.installed);
            let enabled = system.is_some_and(|s| s.enabled);
            let row = || div().flex().items_center().gap_4().p_4().w_full().min_w_0();
            let mut system_row =
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .w_full()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_4()
                            .child(div().flex_1().min_w_0().child(setting_label(
                                t!("settings.system_startup").to_string(),
                                t!("settings.system_startup_hint").to_string(),
                                cx,
                            )))
                            .child(if installed {
                                Switch::new("system-startup")
                                    .checked(enabled)
                                    .disabled(disabled)
                                    .on_click(cx.listener(|this, on, _, _| {
                                        this.engine.set_system_startup(*on)
                                    }))
                                    .into_any_element()
                            } else {
                                Button::new("install-system-startup")
                                    .debug_selector(|| "install-system-startup".into())
                                    .label(t!("settings.system_install").to_string())
                                    .primary()
                                    .disabled(disabled)
                                    .on_click(cx.listener(|this, _, _, _| {
                                        this.engine.set_system_startup(true)
                                    }))
                                    .into_any_element()
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(colors.muted_foreground)
                            .child(
                                t!(if external {
                                    "settings.startup_external"
                                } else if state.busy {
                                    "settings.startup_working"
                                } else if enabled {
                                    "settings.system_registered"
                                } else if installed {
                                    "settings.system_disabled"
                                } else {
                                    "settings.system_pending"
                                })
                                .to_string(),
                            ),
                    );
            if let Some(system) = system.filter(|s| s.installed) {
                if system.version != env!("CARGO_PKG_VERSION") {
                    system_row = system_row.child(
                        div()
                            .text_size(px(12.))
                            .text_color(colors.warning)
                            .child(t!("settings.system_update_required").to_string()),
                    );
                }
                system_row = system_row.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_3()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(12.))
                                .text_color(
                                    if state.approval_outdated || system.approved_devices.is_empty()
                                    {
                                        colors.warning
                                    } else {
                                        colors.muted_foreground
                                    },
                                )
                                .child(if system.approved_devices.is_empty() {
                                    t!("settings.system_no_devices").to_string()
                                } else if state.approval_outdated {
                                    t!("settings.system_approval_outdated").to_string()
                                } else {
                                    t!(
                                        "settings.system_devices",
                                        count = system.approved_devices.len()
                                    )
                                    .to_string()
                                }),
                        )
                        .child(
                            Button::new("refresh-system-approval")
                                .label(t!("settings.system_refresh").to_string())
                                .outline()
                                .disabled(disabled)
                                .on_click(
                                    cx.listener(|this, _, _, _| {
                                        this.engine.update_system_startup()
                                    }),
                                ),
                        ),
                );
                for peer in &system.approved_devices {
                    system_row = system_row.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .text_size(px(12.))
                            .child(peer.name.clone())
                            .child(
                                div()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_color(colors.muted_foreground)
                                    .child(peer.fingerprint.chars().take(16).collect::<String>()),
                            ),
                    );
                }
                system_row = system_row.child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.muted_foreground)
                        .child(t!("settings.system_approval_hint").to_string()),
                );
            }
            let login_toggle = Switch::new("launch-at-login")
                .checked(state.launch_at_login.unwrap_or(false))
                .disabled(disabled || state.launch_at_login.is_none())
                .on_click(cx.listener(|this, on, _, _| this.engine.set_launch_at_login(*on)));
            #[cfg(test)]
            let login_toggle = div()
                .debug_selector(|| "launch-at-login-toggle".into())
                .child(login_toggle);
            let mut content = div().flex().flex_col().gap_3().w_full().min_w_0().child(
                form_group(cx)
                    .child(system_row)
                    .child(Divider::horizontal())
                    .child(
                        row()
                            .child(div().flex_1().min_w_0().child(setting_label(
                                t!("settings.launch_at_login").to_string(),
                                t!("settings.launch_at_login_hint").to_string(),
                                cx,
                            )))
                            .child(login_toggle),
                    ),
            );
            if let Some(error) = state.error {
                content = content.child(
                    div()
                        .flex()
                        .flex_col()
                        .items_start()
                        .gap_2()
                        .min_w_0()
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(colors.danger)
                                .child(error),
                        )
                        .child(
                            Button::new("recheck-startup")
                                .label(t!("settings.startup_recheck").to_string())
                                .outline()
                                .disabled(disabled)
                                .on_click(cx.listener(|this, _, _, _| {
                                    this.engine.refresh_startup_settings()
                                })),
                        ),
                );
            }
            Some(content)
        }
    }
}
