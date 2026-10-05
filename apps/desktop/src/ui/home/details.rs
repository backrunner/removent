use super::*;

/// Connection entry point and local sharing controls share one quiet content surface.
impl HomeView {
    pub(super) fn render_empty_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let connecting = self.connecting.is_some();
        let permissions = |kind: PermissionKind| {
            let granted = match kind {
                PermissionKind::ScreenCapture => self.local_perms.0,
                PermissionKind::Accessibility => self.local_perms.1,
            };
            div()
                .flex()
                .items_center()
                .gap_3()
                .min_h(px(36.))
                .child(div().flex_1().text_size(px(12.)).child(kind.title()))
                .when(granted, |el| {
                    el.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(colors.muted_foreground)
                                    .child(t!("permissions.granted").to_string()),
                            )
                            .child(icon_16("check").text_color(colors.muted_foreground)),
                    )
                })
                .when(!granted, |el| {
                    el.child(
                        Button::new(ElementId::Name(format!("perm-{}", kind.slug()).into()))
                            .label(t!("permissions.open_settings").to_string())
                            .tooltip(kind.purpose())
                            .outline()
                            .small()
                            .h(px(28.))
                            .rounded(px(8.))
                            .bg(colors.secondary_hover.opacity(0.45))
                            .on_click(move |_, _, _| kind.request_and_open_settings()),
                    )
                })
        };
        let daemon_stale = self.daemon_online
            && self
                .daemon_perms
                .is_some_and(|(sr, ax)| (self.local_perms.0 && !sr) || (self.local_perms.1 && !ax));
        div()
            .id("quick-start-scroll")
            .size_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .child(
                // my_auto centers the guide vertically when it fits; the margins
                // collapse to zero once content overflows, so scrolling still works.
                div()
                    .max_w(px(760.))
                    .w_full()
                    .mx_auto()
                    .my_auto()
                    .flex_shrink_0()
                    .p_6()
                    .flex()
                    .flex_col()
                    .gap_5()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_4()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_size(px(11.))
                                    .text_color(colors.muted_foreground)
                                    .child(icon_16("wifi"))
                                    .child(t!("connection.lan_badge").to_string()),
                            )
                            .child(
                                div().flex().flex_col().gap_2().child(
                                    div()
                                        .text_size(px(22.))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child(t!("connection.welcome").to_string()),
                                ),
                            )
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(colors.muted_foreground)
                                    .child(t!("connection.welcome_hint").to_string()),
                            )
                            .child(
                                div().flex().child(
                                    Button::new("open-connection-form")
                                        .h(px(38.))
                                        .icon(icon_16(if connecting { "x" } else { "plus" }))
                                        .label(
                                            t!(if connecting {
                                                "action.cancel"
                                            } else {
                                                "connection.add"
                                            })
                                            .to_string(),
                                        )
                                        .primary()
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            if this.connecting.is_some() {
                                                this.cancel_connection(cx);
                                            } else {
                                                this.open_connection_dialog(None, window, cx);
                                            }
                                        })),
                                ),
                            ),
                    )
                    .child(
                        form_group(cx)
                            .p_5()
                            .gap_4()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_4()
                                    .child(
                                        div()
                                            .flex_1()
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .text_size(px(14.))
                                                    .font_weight(gpui::FontWeight::MEDIUM)
                                                    .child(t!("sharing.title").to_string()),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(12.))
                                                    .text_color(colors.muted_foreground)
                                                    .child(t!("sharing.description").to_string()),
                                            ),
                                    )
                                    .child(
                                        Switch::new("host-switch")
                                            .small()
                                            .checked(self.host_on)
                                            .on_click(
                                                cx.listener(|this, _, _w, cx| this.toggle_host(cx)),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(permissions(PermissionKind::ScreenCapture))
                                    .child(permissions(PermissionKind::Accessibility)),
                            )
                            .when(daemon_stale, |el| {
                                el.child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(colors.warning)
                                        .child(t!("permissions.daemon_restart_hint").to_string()),
                                )
                            })
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .border_t_1()
                                    .border_color(colors.border)
                                    .pt_3()
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(colors.muted_foreground)
                                            .child(t!("device.fingerprint").to_string()),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_size(px(11.))
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .text_color(colors.muted_foreground)
                                            .truncate()
                                            .child(self.my_fp_short.clone()),
                                    )
                                    .child(
                                        Button::new("copy-fp")
                                            .icon(icon_16("copy"))
                                            .ghost()
                                            .small()
                                            .tooltip(t!("device.copy_fingerprint").to_string())
                                            .on_click(cx.listener(|this, _, _w, cx| {
                                                cx.write_to_clipboard(
                                                    gpui::ClipboardItem::new_string(
                                                        this.my_fp_short.clone(),
                                                    ),
                                                );
                                            })),
                                    ),
                            ),
                    ),
            )
    }

    pub(super) fn render_device_detail(
        &self,
        fp: &str,
        row: &DeviceRow,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors;
        let connecting = self.connecting.is_some();
        let fp_c = fp.to_string();
        let fp_short: String = fp.chars().take(8).collect();
        let native = row.protocol == ConnectionProtocol::Removent;
        let trusted = self.trusted.contains(fp);

        let meta_row = |label: String, value: String| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .px_4()
                .py(px(10.))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.muted_foreground)
                        .child(label),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .font_family(cx.theme().mono_font_family.clone())
                        .child(value),
                )
        };

        div()
            .flex()
            .flex_col()
            .gap_6()
            .id("device-detail")
            .size_full()
            .overflow_y_scroll()
            .p_6()
            .max_w(px(760.))
            .mx_auto()
            .child(
                // Device header
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(saved_glyph(row.protocol, 40., &colors))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(20.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(row.name.clone()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(dot(colors.success))
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(colors.muted_foreground)
                                            .child(t!("device.online").to_string()),
                                    ),
                            ),
                    ),
            )
            .child(
                div().flex().gap_2().child(
                    // Cancel immediately releases the form and invalidates queued events.
                    Button::new("connect-device")
                        .icon(icon_16(if connecting { "x" } else { "monitor" }))
                        .label(if connecting {
                            t!("action.cancel").to_string()
                        } else {
                            t!("action.connect").to_string()
                        })
                        .when(!connecting, |b| b.primary())
                        .when(connecting, |b| b.outline())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if this.connecting.is_some() {
                                this.cancel_connection(cx);
                            } else {
                                this.connect_device(&fp_c, window, cx);
                            }
                        })),
                ),
            )
            .child(
                // Device metadata shares the grouped surfaces used by settings.
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(group_title(t!("device.info").to_string(), cx))
                    .child(
                        form_group(cx)
                            .child(meta_row(
                                t!("device.address").to_string(),
                                row.addr.to_string(),
                            ))
                            .child(Divider::horizontal())
                            .child(meta_row(
                                t!("device.protocol").to_string(),
                                row.protocol.short_label().to_string(),
                            ))
                            .when(native, |el| {
                                el.child(Divider::horizontal())
                                    .child(meta_row(t!("device.fingerprint").to_string(), fp_short))
                                    .child(Divider::horizontal())
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .px_4()
                                            .py(px(10.))
                                            .child(
                                                div()
                                                    .text_size(px(12.))
                                                    .text_color(colors.muted_foreground)
                                                    .child(t!("device.trust").to_string()),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(12.))
                                                    .text_color(if trusted {
                                                        colors.success
                                                    } else {
                                                        colors.muted_foreground
                                                    })
                                                    .child(if trusted {
                                                        t!("device.paired").to_string()
                                                    } else {
                                                        t!("device.unpaired").to_string()
                                                    }),
                                            ),
                                    )
                            }),
                    ),
            )
    }

    pub(super) fn render_saved_detail(
        &self,
        entry: &SavedConnection,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors;
        let connecting = self.connecting.is_some();
        let id_connect = entry.id.clone();
        let id_edit = entry.id.clone();
        let id_remove = entry.id.clone();

        let meta_row = |label: String, value: String| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .px_4()
                .py(px(10.))
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(12.))
                        .text_color(colors.muted_foreground)
                        .child(label),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(12.))
                        .font_family(cx.theme().mono_font_family.clone())
                        .child(value),
                )
        };

        div()
            .flex()
            .flex_col()
            .gap_6()
            .id("saved-detail")
            .size_full()
            .overflow_y_scroll()
            .p_6()
            .max_w(px(760.))
            .mx_auto()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(saved_glyph(entry.protocol, 40., &colors))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(20.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(entry.display_name()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(colors.muted_foreground)
                                    .child(t!("saved.subtitle").to_string()),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("connect-saved")
                            .icon(icon_16(if connecting { "x" } else { "monitor" }))
                            .label(if connecting {
                                t!("action.cancel").to_string()
                            } else {
                                t!("action.connect").to_string()
                            })
                            .when(!connecting, |b| b.primary())
                            .when(connecting, |b| b.outline())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if this.connecting.is_some() {
                                    this.cancel_connection(cx);
                                } else {
                                    this.connect_saved(&id_connect, window, cx);
                                }
                            })),
                    )
                    .when(!connecting, |el| {
                        el.child(
                            Button::new("edit-saved")
                                .label(t!("action.edit").to_string())
                                .outline()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if let Some(entry) =
                                        this.saved.iter().find(|s| s.id == id_edit).cloned()
                                    {
                                        this.open_connection_dialog(Some(&entry), window, cx);
                                    }
                                })),
                        )
                        .child(
                            Button::new("remove-saved")
                                .icon(icon_16("trash-2"))
                                .ghost()
                                .tooltip(t!("saved.remove").to_string())
                                .on_click(cx.listener(move |this, _, _w, cx| {
                                    this.remove_saved(&id_remove, cx)
                                })),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(group_title(t!("device.info").to_string(), cx))
                    .child(
                        form_group(cx)
                            .child(meta_row(
                                t!("saved.protocol").to_string(),
                                entry.protocol.label().to_string(),
                            ))
                            .child(Divider::horizontal())
                            .child(meta_row(
                                t!("device.address").to_string(),
                                entry.address().to_string(),
                            ))
                            .when(!entry.username.is_empty(), |el| {
                                el.child(Divider::horizontal()).child(meta_row(
                                    t!("connection.username").to_string(),
                                    entry.username.clone(),
                                ))
                            }),
                    ),
            )
            .when(entry.password_hint, |el| {
                el.child(
                    div()
                        .p_4()
                        .rounded(px(12.))
                        .bg(colors.accent.opacity(0.08))
                        .text_size(px(12.))
                        .text_color(colors.muted_foreground)
                        .child(t!("saved.credentials_hint").to_string()),
                )
            })
    }
}
