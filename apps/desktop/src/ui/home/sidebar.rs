use super::*;
use gpui_component::menu::{ContextMenuExt, PopupMenuItem};

impl HomeView {
    pub(super) fn saved_discovery_match(&self, row: &DeviceRow) -> Option<&SavedConnection> {
        let address = removent_client::connection::ConnectionAddress::from_socket(row.addr);
        self.saved
            .iter()
            .find(|entry| entry.matches_discovered(row.protocol, &address, &row.hosts))
    }

    pub(super) fn reconcile_discovery_selection(&mut self) {
        if let Some(Selection::Device(id)) = &self.selected
            && let Some(row) = self.devices.get(id)
            && let Some(saved) = self.saved_discovery_match(row)
        {
            self.selected = Some(Selection::Saved(saved.id.clone()));
        }
    }

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
                            .outline()
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
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.reset_authentication_form(window, cx);
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
        text_width: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors;
        let selected =
            !self.settings_open && matches!(&self.selected, Some(Selection::Device(s)) if s == fp);
        let fp_sel = fp.to_string();
        let fp_dbl = fp.to_string();
        let mut el = div()
            .id(gpui::ElementId::Name(format!("dev-{fp}").into()))
            .debug_selector({
                let id = fp.to_string();
                move || format!("discovered-{id}")
            })
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
            .rounded(px(10.))
            .cursor_pointer()
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
            .child(sidebar_labels(
                text_width,
                row.name.clone(),
                row.protocol.short_label(),
                row.addr.to_string(),
                cx,
            ))
            .when(trusted, |el| {
                el.child(
                    icon_16("shield-check")
                        .flex_shrink_0()
                        .text_color(colors.muted_foreground),
                )
            });
        if selected {
            el = el
                .bg(colors.accent.opacity(0.15))
                .border_color(colors.accent.opacity(0.3))
                .hover(|s| {
                    s.bg(colors.accent.opacity(0.22))
                        .border_color(colors.accent.opacity(0.35))
                })
                .active(|s| {
                    s.bg(colors.accent.opacity(0.28))
                        .border_color(colors.accent.opacity(0.4))
                });
        } else {
            el = el
                .hover(|s| {
                    s.bg(colors.list_hover)
                        .border_color(colors.border.opacity(0.5))
                })
                .active(|s| {
                    s.bg(colors.list_active)
                        .border_color(colors.border.opacity(0.8))
                });
        }
        let target = self.discovered_wake_target(row);
        let home = cx.weak_entity();
        el.context_menu(move |menu, _, _| HomeView::wake_menu(menu, target.clone(), home.clone()))
    }

    pub(super) fn render_saved_row(
        &self,
        entry: &SavedConnection,
        text_width: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors;
        let selected = !self.settings_open
            && matches!(&self.selected, Some(Selection::Saved(id)) if id == &entry.id);
        let id_sel = entry.id.clone();
        let id_dbl = entry.id.clone();
        let mut el = div()
            .id(gpui::ElementId::Name(format!("saved-{}", entry.id).into()))
            .debug_selector({
                let id = entry.id.clone();
                move || format!("bookmark-{id}")
            })
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
            .rounded(px(10.))
            .cursor_pointer()
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
            .child(sidebar_labels(
                text_width,
                entry.display_name(),
                entry.protocol.short_label(),
                entry.address().to_string(),
                cx,
            ));
        if selected {
            el = el
                .bg(colors.accent.opacity(0.15))
                .border_color(colors.accent.opacity(0.3))
                .hover(|s| {
                    s.bg(colors.accent.opacity(0.22))
                        .border_color(colors.accent.opacity(0.35))
                })
                .active(|s| {
                    s.bg(colors.accent.opacity(0.28))
                        .border_color(colors.accent.opacity(0.4))
                });
        } else {
            el = el
                .hover(|s| {
                    s.bg(colors.list_hover)
                        .border_color(colors.border.opacity(0.5))
                })
                .active(|s| {
                    s.bg(colors.list_active)
                        .border_color(colors.border.opacity(0.8))
                });
        }
        let target = entry.clone();
        let home = cx.weak_entity();
        el.context_menu(move |menu, _, cx| {
            let busy = home.upgrade().is_none_or(|home| {
                let home = home.read(cx);
                home.connecting.is_some() || home.connection_save_task.is_some()
            });
            let edit_home = home.clone();
            let edit_id = target.id.clone();
            let remove_home = home.clone();
            let remove_id = target.id.clone();
            HomeView::wake_menu(menu, target.clone(), home.clone())
                .separator()
                .item(
                    PopupMenuItem::new(t!("action.edit").to_string())
                        .disabled(busy)
                        .on_click(move |_, window, cx| {
                            let _ = edit_home.update(cx, |this, cx| {
                                if this.connecting.is_none()
                                    && this.connection_save_task.is_none()
                                    && let Some(entry) =
                                        this.saved.iter().find(|s| s.id == edit_id).cloned()
                                {
                                    this.open_connection_dialog(Some(&entry), window, cx);
                                }
                            });
                        }),
                )
                .item(
                    PopupMenuItem::new(t!("saved.remove").to_string())
                        .icon(icon_16("trash-2"))
                        .disabled(busy)
                        .on_click(move |_, _, cx| {
                            let _ = remove_home.update(cx, |this, cx| {
                                if this.connecting.is_none() && this.connection_save_task.is_none()
                                {
                                    this.remove_saved(&remove_id, cx);
                                }
                            });
                        }),
                )
        })
    }

    pub(super) fn render_sidebar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if self.settings_open {
            return self.render_settings_sidebar(window, cx);
        }
        let colors = cx.theme().colors;
        let sidebar_width = if window.viewport_size().width < px(1000.) {
            228.
        } else {
            256.
        };
        // Explicit widths also let GPUI remeasure one-line labels after resizing.
        let row_text_width = sidebar_width - 1. - 2. * SIDEBAR_INSET - 32. - 12.;
        let discovery = self.engine.settings().discovery;
        let discovering = discovery.removent || discovery.vnc || discovery.rdp;
        let query = self.search_input.read(cx).value().trim().to_lowercase();
        let rows: Vec<_> = self
            .devices
            .iter()
            .filter(|(_, row)| self.saved_discovery_match(row).is_none())
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
        // Keep both sources visible, including their own empty/search states.
        let mut saved_section = div()
            .id("saved-section")
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap_1()
            .child(list_group_label(
                t!("saved.section").to_string(),
                saved_rows.len(),
                cx,
            ));
        if saved_rows.is_empty() {
            saved_section = saved_section.child(
                div()
                    .px(px(SIDEBAR_ROW_INSET))
                    .py_2()
                    .text_size(px(12.))
                    .text_color(colors.muted_foreground)
                    .child(
                        t!(if query.is_empty() {
                            "saved.empty"
                        } else {
                            "saved.no_match"
                        })
                        .to_string(),
                    ),
            );
        }
        for entry in saved_rows {
            saved_section = saved_section.child(self.render_saved_row(entry, row_text_width, cx));
        }
        let mut nearby_section = div()
            .id("discovery-section")
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap_1()
            .mt_3()
            .child(div().mx(px(SIDEBAR_ROW_INSET)).h(px(1.)).bg(colors.border))
            .child(list_group_label(
                t!("device.nearby").to_string(),
                rows.len(),
                cx,
            ));
        if rows.is_empty() {
            nearby_section = nearby_section.child(
                div()
                    .mx(px(SIDEBAR_ROW_INSET))
                    .p_3()
                    .mt_1()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(colors.border)
                    .bg(colors.group_box)
                    .flex()
                    .flex_col()
                    .gap_2()
                    .text_color(colors.muted_foreground)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .size(px(32.))
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(8.))
                                    .bg(colors.secondary_hover)
                                    .child(
                                        icon_16(if !query.is_empty() {
                                            "search"
                                        } else if discovering {
                                            "wifi"
                                        } else {
                                            "wifi-off"
                                        })
                                        .size(px(20.))
                                        .text_color(
                                            if discovering && query.is_empty() {
                                                colors.accent
                                            } else {
                                                colors.muted_foreground
                                            },
                                        ),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(12.))
                                    .line_height(px(18.))
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
                            ),
                    )
                    .when(query.is_empty(), |el| {
                        el.child(
                            div().text_size(px(11.)).line_height(px(17.)).child(
                                t!(if discovering {
                                    "device.searching_hint"
                                } else {
                                    "device.discovery_off_hint"
                                })
                                .to_string(),
                            ),
                        )
                    })
                    .when(query.is_empty() && !discovering, |el| {
                        el.child(
                            div().flex().justify_end().pt_1().child(
                                div()
                                    .flex_shrink_0()
                                    .debug_selector(|| "enable-discovery".into())
                                    .child(
                                        Button::new("enable-discovery")
                                            .icon(icon_16("wifi"))
                                            .label(t!("device.enable_discovery").to_string())
                                            .primary()
                                            .small()
                                            .h(px(28.))
                                            .px_3()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                if this.persist_settings(|s| {
                                                    s.discovery.removent = true;
                                                    s.discovery.vnc = true;
                                                    s.discovery.rdp = true;
                                                }) {
                                                    this.set_status(
                                                        t!("device.discovery_enabled").to_string(),
                                                        StatusTone::Ok,
                                                    );
                                                }
                                                cx.notify();
                                            })),
                                    ),
                            ),
                        )
                    }),
            );
        }
        for (fp, row) in rows {
            let trusted = self.trusted.contains(fp.as_str());
            let text_width = row_text_width - if trusted { 16. + 12. } else { 0. };
            nearby_section =
                nearby_section.child(self.render_device_row(fp, row, trusted, text_width, cx));
        }
        let list = div()
            .id("device-list")
            .flex()
            .flex_col()
            .flex_1()
            .w_full()
            .min_w_0()
            .min_h_0()
            .pb_3()
            .overflow_y_scroll()
            .child(saved_section)
            .child(nearby_section);
        div()
            .w(px(sidebar_width))
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
                    .child(section_label(t!("device.section").to_string(), cx)),
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
                    .border_t_1()
                    .border_color(colors.border)
                    .px(px(SIDEBAR_INSET - SIDEBAR_ROW_INSET))
                    .py_2()
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
                            .py_2()
                            .rounded(px(10.))
                            .when(self.selected.is_none() && !self.settings_open, |el| {
                                el.bg(colors.accent.opacity(0.15))
                                    .border_color(colors.accent.opacity(0.3))
                            })
                            .hover(|el| {
                                if self.selected.is_none() && !self.settings_open {
                                    el.bg(colors.accent.opacity(0.22))
                                        .border_color(colors.accent.opacity(0.35))
                                } else {
                                    el.bg(colors.list_hover)
                                        .border_color(colors.border.opacity(0.5))
                                }
                            })
                            .active(|el| {
                                if self.selected.is_none() && !self.settings_open {
                                    el.bg(colors.accent.opacity(0.28))
                                        .border_color(colors.accent.opacity(0.4))
                                } else {
                                    el.bg(colors.list_active)
                                        .border_color(colors.border.opacity(0.8))
                                }
                            })
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.selected = None;
                                this.settings_open = false;
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .size(px(32.))
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(8.))
                                    .bg(colors.secondary_hover.opacity(0.45))
                                    .child(icon_16("monitor").text_color(colors.muted_foreground)),
                            )
                            .child(
                                div()
                                    .w(px(row_text_width))
                                    .flex_shrink_0()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.))
                                    .child(
                                        sidebar_line()
                                            .w(px(row_text_width))
                                            .h(px(16.))
                                            .text_size(px(13.))
                                            .line_height(px(16.))
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .child(self.engine.device_name()),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .h(px(18.))
                                            .items_center()
                                            .gap_1()
                                            .child(sidebar_tag(
                                                t!("device.this_mac").to_string(),
                                                false,
                                                cx,
                                            ))
                                            .child(sidebar_tag(
                                                t!(if self.host_on {
                                                    "sharing.enabled"
                                                } else {
                                                    "sharing.disabled"
                                                })
                                                .to_string(),
                                                self.host_on,
                                                cx,
                                            )),
                                    ),
                            ),
                    ),
            )
    }
}
