use super::*;

impl HomeView {
    pub(super) fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let settings = self.engine.settings();
        let mut navigation = div()
            .flex()
            .flex_wrap()
            .gap(px(2.))
            .p(px(3.))
            .w_auto()
            .max_w_full()
            .rounded(px(10.))
            .bg(colors.sidebar)
            .border_1()
            .border_color(colors.border);
        navigation.style().align_self = Some(gpui::AlignSelf::FlexStart);
        for (index, name, key) in [
            (0, "settings", "settings.general"),
            (1, "sun", "settings.appearance"),
            (2, "shield-check", "settings.security"),
            (3, "wifi", "settings.sharing"),
            (5, "search", "settings.discovery"),
            (6, "cloud", "sync.title"),
            (4, "info", "settings.system"),
        ] {
            navigation = navigation.child(
                Button::new(("settings-tab", index))
                    .ghost()
                    .icon(icon_16(name))
                    .label(t!(key).to_string())
                    .h(px(32.))
                    .rounded(px(7.))
                    .tab(self.settings_section == index)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings_section = index;
                        cx.notify();
                    })),
            );
        }
        let row = || div().flex().flex_col().gap_3().p_5().w_full().min_w_0();
        let header = |name, title, description| {
            section_header(name, t!(title).to_string(), t!(description).to_string(), cx)
        };
        let mut content = div().flex().flex_col().gap_5().w_full().min_w_0();
        match self.settings_section {
            0 => {
                content = content
                    .child(header(
                        "monitor",
                        "settings.general",
                        "settings.general_description",
                    ))
                    .child(
                        form_group(cx).child(
                            row()
                                .child(setting_label(
                                    t!("settings.device_name").to_string(),
                                    t!("settings.device_name_hint").to_string(),
                                    cx,
                                ))
                                .child(
                                    div()
                                        .flex()
                                        .w_full()
                                        .items_center()
                                        .gap_3()
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .child(form_input(&self.device_name_input)),
                                        )
                                        .child(
                                            Button::new("save-device-name")
                                                .label(t!("action.save").to_string())
                                                .primary()
                                                .h(px(36.))
                                                .flex_shrink_0()
                                                .disabled(
                                                    self.device_name_input
                                                        .read(cx)
                                                        .value()
                                                        .trim()
                                                        .is_empty()
                                                        || self
                                                            .device_name_input
                                                            .read(cx)
                                                            .value()
                                                            .trim()
                                                            == settings.device_name,
                                                )
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.save_device_name(cx)
                                                })),
                                        ),
                                ),
                        ),
                    )
                    .child(
                        form_group(cx).child(
                            row()
                                .child(setting_label(
                                    t!("settings.language").to_string(),
                                    t!("settings.language_hint").to_string(),
                                    cx,
                                ))
                                .child({
                                    let view = cx.entity();
                                    segmented(
                                        "language",
                                        &[
                                            t!("settings.language.system").to_string(),
                                            "English".into(),
                                            "中文".into(),
                                        ],
                                        match settings.language {
                                            Language::System => 0,
                                            Language::En => 1,
                                            Language::ZhCn => 2,
                                        },
                                        Rc::new(move |i, _, window, app| {
                                            view.update(app, |this, cx| {
                                                this.set_language(
                                                    match i {
                                                        1 => Language::En,
                                                        2 => Language::ZhCn,
                                                        _ => Language::System,
                                                    },
                                                    window,
                                                    cx,
                                                )
                                            });
                                        }),
                                        cx,
                                    )
                                }),
                        ),
                    );
            }
            1 => {
                content = content
                    .child(header(
                        "sun",
                        "settings.appearance",
                        "settings.appearance_description",
                    ))
                    .child(
                        form_group(cx).child(
                            row()
                                .child(setting_label(
                                    t!("settings.color_mode").to_string(),
                                    t!("settings.theme_hint").to_string(),
                                    cx,
                                ))
                                .child({
                                    let view = cx.entity();
                                    segmented(
                                        "theme",
                                        &[
                                            t!("settings.theme.system").to_string(),
                                            t!("settings.theme.dark").to_string(),
                                            t!("settings.theme.light").to_string(),
                                        ],
                                        match settings.theme {
                                            ThemePref::System => 0,
                                            ThemePref::Dark => 1,
                                            ThemePref::Light => 2,
                                        },
                                        Rc::new(move |i, _, window, app| {
                                            view.update(app, |this, cx| {
                                                this.set_theme(
                                                    match i {
                                                        1 => ThemePref::Dark,
                                                        2 => ThemePref::Light,
                                                        _ => ThemePref::System,
                                                    },
                                                    window,
                                                    cx,
                                                )
                                            });
                                        }),
                                        cx,
                                    )
                                })
                                .child(
                                    div()
                                        .mt_2()
                                        .p_4()
                                        .rounded(px(12.))
                                        .bg(colors.background)
                                        .border_1()
                                        .border_color(colors.border)
                                        .child(section_header(
                                            "monitor",
                                            "Removent".into(),
                                            t!("settings.preview_hint").to_string(),
                                            cx,
                                        ))
                                        .child(
                                            div()
                                                .mt_4()
                                                .flex()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .w(px(72.))
                                                        .h(px(54.))
                                                        .rounded(px(8.))
                                                        .bg(colors.sidebar),
                                                )
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .h(px(54.))
                                                        .rounded(px(8.))
                                                        .bg(colors.group_box)
                                                        .shadow_sm()
                                                        .child(
                                                            div()
                                                                .m_3()
                                                                .w(px(48.))
                                                                .h(px(6.))
                                                                .rounded_full()
                                                                .bg(colors.accent),
                                                        ),
                                                ),
                                        ),
                                ),
                        ),
                    );
            }
            2 => {
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
                                                        t!("status.admission_mode_updated")
                                                            .to_string(),
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
                                                AdmissionMode::AlwaysAsk =>
                                                    "settings.admission.ask_hint",
                                                AdmissionMode::TrustedAuto =>
                                                    "settings.admission.trusted_hint",
                                                AdmissionMode::DenyAll =>
                                                    "settings.admission.deny_hint",
                                            })
                                            .to_string(),
                                        ),
                                ),
                        ),
                    );
            }
            3 => {
                content = content
                    .child(header(
                        "wifi",
                        "settings.sharing",
                        "settings.sharing_description",
                    ))
                    .child(
                        form_group(cx)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_4()
                                    .p_5()
                                    .child(
                                        setting_label(
                                            t!("settings.vnc_enabled").to_string(),
                                            format!(
                                                "Apple Remote Desktop · VNC · TCP {}",
                                                settings.vnc_port
                                            ),
                                            cx,
                                        )
                                        .flex_1(),
                                    )
                                    .child(
                                        Switch::new("vnc-enabled")
                                            .checked(settings.vnc_enabled)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                if this.persist_settings(|s| {
                                                    s.vnc_enabled = !s.vnc_enabled
                                                }) {
                                                    this.set_status(
                                                        t!("status.vnc_updated").to_string(),
                                                        StatusTone::Ok,
                                                    );
                                                }
                                                cx.notify();
                                            })),
                                    ),
                            )
                            .when(settings.vnc_enabled, |el| {
                                el.child(Divider::horizontal()).child(
                                    row()
                                        .child(setting_label(
                                            t!("settings.vnc_password").to_string(),
                                            t!("settings.vnc_hint").to_string(),
                                            cx,
                                        ))
                                        .child(form_input(&self.vnc_password_input).mask_toggle())
                                        .child(
                                            div().flex().justify_end().child(
                                                Button::new("save-vnc")
                                                    .label(t!("action.save").to_string())
                                                    .primary()
                                                    .h(px(36.))
                                                    .disabled(
                                                        self.vnc_password_input
                                                            .read(cx)
                                                            .value()
                                                            .as_str()
                                                            == settings.vnc_password,
                                                    )
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.save_vnc(cx)
                                                    })),
                                            ),
                                        ),
                                )
                            }),
                    )
                    .child(
                        div()
                            .p_4()
                            .rounded(px(12.))
                            .bg(colors.accent.opacity(0.08))
                            .text_size(px(12.))
                            .text_color(colors.muted_foreground)
                            .child(t!("settings.sharing_scope_hint").to_string()),
                    );
            }
            5 => {
                content = content.child(header(
                    "search",
                    "settings.discovery",
                    "settings.discovery_description",
                ));
                let mut group = form_group(cx);
                for (index, protocol) in ConnectionProtocol::ALL.into_iter().enumerate() {
                    if index > 0 {
                        group = group.child(Divider::horizontal());
                    }
                    group = group.child(
                        div().flex().items_center().gap_4().p_5()
                            .child(div().flex_1().text_size(px(13.)).child(protocol.label()))
                            .child(div().flex().flex_shrink_0()
                                .debug_selector(move || format!("discovery-enabled-{index}"))
                                .child(Switch::new(("discovery-enabled", index))
                                .checked(crate::discovery::enabled(settings.discovery, protocol))
                                .on_click(cx.listener(move |this, checked, _, cx| {
                                    if this.persist_settings(|s| match protocol {
                                        ConnectionProtocol::Removent => s.discovery.removent = *checked,
                                        ConnectionProtocol::Vnc => s.discovery.vnc = *checked,
                                        ConnectionProtocol::Rdp => s.discovery.rdp = *checked,
                                    }) && !*checked {
                                        this.devices.retain(|_, row| row.protocol != protocol);
                                        if matches!(&this.selected, Some(Selection::Device(id)) if !this.devices.contains_key(id)) {
                                            this.selected = None;
                                        }
                                    }
                                    cx.notify();
                                }))))
                    );
                }
                content = content.child(group).child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.muted_foreground)
                        .child(t!("settings.discovery_hint").to_string()),
                );
            }
            6 => {
                content = content.child(self.render_cloud_sync(cx));
            }
            _ => {
                content = content
                    .child(header(
                        "info",
                        "settings.system",
                        "settings.system_description",
                    ))
                    .child(
                        form_group(cx).child(
                            row()
                                .child(setting_label(
                                    t!("settings.data_dir").to_string(),
                                    t!("settings.data_hint").to_string(),
                                    cx,
                                ))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_3()
                                        .w_full()
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .px_3()
                                                .py_2()
                                                .rounded(px(8.))
                                                .bg(colors.background)
                                                .text_size(px(11.))
                                                .font_family(cx.theme().mono_font_family.clone())
                                                .text_color(colors.muted_foreground)
                                                .truncate()
                                                .child(
                                                    self.engine.data_dir().display().to_string(),
                                                ),
                                        )
                                        .child(
                                            Button::new("open-data-dir")
                                                .icon(icon_16("folder"))
                                                .label(t!("action.open").to_string())
                                                .outline()
                                                .h(px(36.))
                                                .flex_shrink_0()
                                                .on_click(cx.listener(|this, _, _, _| {
                                                    let _ = std::process::Command::new("open")
                                                        .arg(this.engine.data_dir())
                                                        .spawn();
                                                })),
                                        ),
                                ),
                        ),
                    )
                    .child(
                        form_group(cx).child(
                            row()
                                .child(setting_label(
                                    t!("settings.logs").to_string(),
                                    t!("settings.logs_hint").to_string(),
                                    cx,
                                ))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_3()
                                        .w_full()
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .px_3()
                                                .py_2()
                                                .rounded(px(8.))
                                                .bg(colors.background)
                                                .text_size(px(11.))
                                                .font_family(cx.theme().mono_font_family.clone())
                                                .text_color(colors.muted_foreground)
                                                .truncate()
                                                .child(self.engine.data_dir().join("logs").display().to_string()),
                                        )
                                        .child(
                                            Button::new("open-logs-dir")
                                                .icon(icon_16("folder"))
                                                .label(t!("settings.open_logs").to_string())
                                                .outline()
                                                .h(px(36.))
                                                .flex_shrink_0()
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    let dir = this.engine.data_dir().join("logs");
                                                    let result = std::fs::create_dir_all(&dir).and_then(|()| {
                                                        std::process::Command::new("open").arg(&dir).spawn()
                                                    });
                                                    if let Err(error) = result {
                                                        tracing::warn!(%error, "could not open logs folder");
                                                        this.set_status(
                                                            t!("settings.open_logs_failed", err = error.to_string()),
                                                            StatusTone::Err,
                                                        );
                                                        cx.notify();
                                                    }
                                                })),
                                        ),
                                ),
                        ),
                    )
                    .child(self.render_update_section(cx));
            }
        }
        div()
            .id("settings")
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_6()
                    .w_full()
                    .max_w(px(760.))
                    .mx_auto()
                    .p_6()
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .justify_between()
                            .gap_3()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_size(px(24.))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .child(t!("settings.title").to_string()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(colors.muted_foreground)
                                            .child(t!("settings.subtitle").to_string()),
                                    ),
                            )
                            .child(
                                Button::new("close-settings")
                                    .icon(icon_16("x"))
                                    .outline()
                                    .small()
                                    .tooltip(t!("action.back").to_string())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.settings_open = false;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(navigation)
                    .flex_shrink_0()
                    .pb_4(),
            )
            .child(
                div()
                    .id(("settings-content", self.settings_section))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .w_full()
                            .max_w(px(760.))
                            .mx_auto()
                            .px_6()
                            .pb_6()
                            .pt_2()
                            .child(content),
                    ),
            )
    }
}
