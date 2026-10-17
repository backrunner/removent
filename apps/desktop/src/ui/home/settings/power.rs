use super::*;

impl HomeView {
    pub(super) fn render_power_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let enabled = self.daemon_power.as_ref().map_or(
            self.engine.settings().prevent_idle_sleep,
            |(enabled, _, _)| *enabled,
        );
        let active = self
            .daemon_power
            .as_ref()
            .is_some_and(|(on, active, _)| *on && *active);
        let failed = self
            .daemon_power
            .as_ref()
            .is_some_and(|(_, _, error)| error.is_some());
        let switch = Switch::new("prevent-idle-sleep")
            .checked(enabled)
            .on_click(cx.listener(|this, checked, _, cx| {
                if this.persist_settings(|s| s.prevent_idle_sleep = *checked) {
                    this.daemon_power = None;
                    this.engine.refresh_host_status();
                }
                cx.notify();
            }));
        #[cfg(test)]
        let switch = div()
            .debug_selector(|| "prevent-idle-sleep-toggle".into())
            .child(switch);
        form_group(cx).child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .p_4()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(setting_label(
                            t!("settings.prevent_idle_sleep").to_string(),
                            t!("settings.prevent_idle_sleep_hint").to_string(),
                            cx,
                        ))
                        .when(enabled, |el| {
                            el.child(
                                div()
                                    .mt_2()
                                    .text_size(px(12.))
                                    .text_color(cx.theme().colors.muted_foreground)
                                    .child(
                                        t!(if failed {
                                            "settings.power_failed"
                                        } else if active {
                                            "settings.power_active"
                                        } else {
                                            "settings.power_pending"
                                        })
                                        .to_string(),
                                    ),
                            )
                        }),
                )
                .child(switch),
        )
    }
}
