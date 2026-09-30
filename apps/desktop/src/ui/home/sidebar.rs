use super::*;

impl HomeView {
    pub(super) fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        TitleBar::new()
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child("Removent"),
            )
            .child(
                div()
                    .pr_2()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        Button::new("add-connection")
                            .icon(icon_16("plus"))
                            .label(t!("connection.add").to_string())
                            .primary()
                            .small()
                            .tooltip(t!("connection.add").to_string())
                            .disabled(self.connecting.is_some())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_connection_dialog(None, window, cx)
                            })),
                    )
                    .child(
                        Button::new("open-settings")
                            .icon(icon_16("settings"))
                            .ghost()
                            .small()
                            .tooltip(t!("settings.title").to_string())
                            .tab(self.settings_open)
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.settings_open = !this.settings_open;
                                cx.notify();
                            })),
                    ),
            )
    }

    pub(super) fn render_device_row(
        &self,
        fp: &str,
        row: &DeviceRow,
        trusted: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors;
        let selected =
            !self.settings_open && matches!(&self.selected, Some(Selection::Device(s)) if s == fp);
        let fp_sel = fp.to_string();
        let fp_dbl = fp.to_string();
        let mut el = div()
            .id(gpui::ElementId::Name(format!("dev-{fp}").into()))
            .tab_index(0)
            .border_1()
            .border_color(colors.border.opacity(0.))
            .focus(|style| style.border_color(colors.ring))
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_3()
            .px(px(SIDEBAR_ROW_INSET - 1.))
            .py_2()
            .rounded(px(12.))
            .cursor_default()
            // Single-click selects; double-click connects directly.
            .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, window, cx| {
                if ev.click_count() >= 2 {
                    this.connect_device(&fp_dbl, window, cx);
                } else {
                    this.selected = Some(Selection::Device(fp_sel.clone()));
                    this.settings_open = false;
                    cx.notify();
                }
            }))
            .child(saved_glyph(row.protocol, 32., &colors))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        sidebar_line()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(row.name.clone()),
                    )
                    .child(
                        sidebar_line()
                            .text_size(px(11.))
                            .text_color(colors.muted_foreground)
                            .child(format!("{} · {}", row.protocol.short_label(), row.addr)),
                    ),
            )
            .when(trusted, |el| {
                el.child(
                    icon_16("shield-check")
                        .flex_shrink_0()
                        .text_color(colors.muted_foreground),
                )
            });
        if selected {
            el = el
                .bg(colors.accent.opacity(0.12))
                .border_color(colors.accent.opacity(0.25))
                .shadow_sm()
                .hover(|s| s.bg(colors.accent.opacity(0.20)))
                .active(|s| s.bg(colors.accent.opacity(0.28)));
        } else {
            el = el
                .hover(|s| s.bg(colors.list_hover))
                .active(|s| s.bg(colors.list_active));
        }
        el
    }

    pub(super) fn render_saved_row(
        &self,
        entry: &SavedConnection,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors;
        let selected = !self.settings_open
            && matches!(&self.selected, Some(Selection::Saved(id)) if id == &entry.id);
        let id_sel = entry.id.clone();
        let id_dbl = entry.id.clone();
        let mut el = div()
            .id(gpui::ElementId::Name(format!("saved-{}", entry.id).into()))
            .tab_index(0)
            .border_1()
            .border_color(colors.border.opacity(0.))
            .focus(|style| style.border_color(colors.ring))
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_3()
            .px(px(SIDEBAR_ROW_INSET - 1.))
            .py_2()
            .rounded(px(12.))
            .cursor_default()
            // Same interaction as discovered devices: click selects, double-click connects.
            .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, window, cx| {
                if ev.click_count() >= 2 {
                    this.connect_saved(&id_dbl, window, cx);
                } else {
                    this.selected = Some(Selection::Saved(id_sel.clone()));
                    this.settings_open = false;
                    cx.notify();
                }
            }))
            .child(saved_glyph(entry.protocol, 32., &colors))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        sidebar_line()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(entry.display_name()),
                    )
                    .child(
                        sidebar_line()
                            .text_size(px(11.))
                            .text_color(colors.muted_foreground)
                            .child(format!(
                                "{} · {}",
                                entry.protocol.short_label(),
                                entry.address()
                            )),
                    ),
            );
        if selected {
            el = el
                .bg(colors.accent.opacity(0.12))
                .border_color(colors.accent.opacity(0.25))
                .shadow_sm()
                .hover(|s| s.bg(colors.accent.opacity(0.20)))
                .active(|s| s.bg(colors.accent.opacity(0.28)));
        } else {
            el = el
                .hover(|s| s.bg(colors.list_hover))
                .active(|s| s.bg(colors.list_active));
        }
        el
    }

    pub(super) fn render_sidebar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors;
        let discovery = self.engine.settings().discovery;
        let discovering = discovery.removent || discovery.vnc || discovery.rdp;
        let query = self.search_input.read(cx).value().trim().to_lowercase();
        let rows: Vec<_> = self
            .devices
            .iter()
            .filter(|(_, row)| {
                query.is_empty()
                    || row.name.to_lowercase().contains(&query)
                    || row.addr.to_string().contains(&query)
                    || row.protocol.short_label().to_lowercase().contains(&query)
            })
            .collect();
        let saved_rows: Vec<_> = self
            .saved
            .iter()
            .filter(|entry| {
                query.is_empty()
                    || entry.name.to_lowercase().contains(&query)
                    || entry.host.to_lowercase().contains(&query)
                    || entry.address().to_string().contains(&query)
            })
            .collect();
        let mut list = div()
            .id("device-list")
            .flex()
            .flex_col()
            .flex_1()
            .w_full()
            .min_w_0()
            .min_h_0()
            .gap_1()
            .overflow_y_scroll();
        // Bookmarks come first: they are the user's own connect targets, while
        // discovered devices below depend on LAN presence.
        if !saved_rows.is_empty() {
            list = list.child(list_group_label(t!("saved.section").to_string(), cx));
            for entry in saved_rows.iter().copied() {
                list = list.child(self.render_saved_row(entry, cx));
            }
            if !rows.is_empty() {
                list = list.child(list_group_label(t!("device.nearby").to_string(), cx));
            }
        }
        if rows.is_empty() && (saved_rows.is_empty() || query.is_empty()) {
            list = list.child(
                div()
                    .p_4()
                    .mx(px(SIDEBAR_ROW_INSET))
                    .mt_2()
                    .rounded(px(14.))
                    .border_1()
                    .border_color(colors.border)
                    .bg(colors.group_box)
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .mb_1()
                            .child(icon("wifi").size(px(24.)).text_color(colors.accent)),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(colors.muted_foreground)
                            .child(
                                t!(if query.is_empty() {
                                    if discovering {
                                        "device.searching"
                                    } else {
                                        "device.discovery_off"
                                    }
                                } else {
                                    "device.no_match"
                                })
                                .to_string(),
                            ),
                    )
                    .when(query.is_empty(), |el| {
                        el.child(
                            div()
                                .text_size(px(12.))
                                .text_color(colors.muted_foreground)
                                .child(
                                    t!(if discovering {
                                        "device.searching_hint"
                                    } else {
                                        "device.discovery_off_hint"
                                    })
                                    .to_string(),
                                ),
                        )
                    }),
            );
        }
        for (fp, row) in rows {
            list =
                list.child(self.render_device_row(fp, row, self.trusted.contains(fp.as_str()), cx));
        }
        div()
            .w(px(if window.viewport_size().width < px(1000.) {
                228.
            } else {
                256.
            }))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(colors.border)
            .bg(colors.sidebar)
            .child(
                div()
                    .px(px(SIDEBAR_INSET))
                    .pt_5()
                    .pb_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(section_label(t!("device.section").to_string(), cx))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(colors.muted_foreground)
                            .px_2()
                            .py_1()
                            .rounded(px(6.))
                            .bg(colors.secondary)
                            .child(self.devices.len().to_string()),
                    ),
            )
            .child(
                div().px(px(SIDEBAR_INSET)).pb_3().child(
                    form_input(&self.search_input)
                        .prefix(icon_16("search"))
                        .cleanable(true),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .px(px(SIDEBAR_INSET - SIDEBAR_ROW_INSET))
                    .child(list),
            )
            .child(
                div()
                    .px(px(SIDEBAR_INSET - SIDEBAR_ROW_INSET))
                    .py_3()
                    .child(
                        div()
                            .id("local-device")
                            .tab_index(0)
                            .border_1()
                            .border_color(colors.border.opacity(0.))
                            .focus(|style| style.border_color(colors.ring))
                            .flex()
                            .items_center()
                            .gap_3()
                            .w_full()
                            .min_w_0()
                            .px(px(SIDEBAR_ROW_INSET - 1.))
                            .py_3()
                            .rounded(px(12.))
                            .when(self.selected.is_none() && !self.settings_open, |el| {
                                el.bg(colors.accent.opacity(0.12))
                                    .border_color(colors.accent.opacity(0.2))
                            })
                            .hover(|el| el.bg(colors.list_hover))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.selected = None;
                                this.settings_open = false;
                                cx.notify();
                            }))
                            .child(device_glyph(32., &colors))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        sidebar_line()
                                            .text_size(px(12.))
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .child(t!("device.this_mac").to_string()),
                                    )
                                    .child(
                                        sidebar_line()
                                            .text_size(px(11.))
                                            .text_color(colors.muted_foreground)
                                            .child(self.engine.device_name()),
                                    ),
                            )
                            .child(dot(if self.host_on {
                                colors.success
                            } else {
                                colors.muted_foreground.opacity(0.5)
                            })),
                    ),
            )
    }
}
