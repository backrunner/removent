use super::timestamps::local_clock_time;
use super::*;

impl HomeView {
    pub(super) fn update_cloud_sync(
        &mut self,
        command: removent_client::cloud_sync::Command,
        cx: &mut Context<Self>,
    ) {
        match self.engine.cloud_sync_command(command) {
            Ok(status) => {
                self.cloud_sync_status = status;
                self.saved = self.engine.saved_connections();
                self.reconcile_discovery_selection();
            }
            Err(error) => self.set_status(
                t!("status.settings_save_failed", err = error).to_string(),
                StatusTone::Err,
            ),
        }
        cx.notify();
    }

    pub(super) fn render_cloud_sync(&self, cx: &mut Context<Self>) -> Div {
        use removent_client::cloud_sync::Command;
        let enabled = self.cloud_sync_status["enabled"].as_bool().unwrap_or(false);
        let code = self.cloud_sync_status["code"].as_str().unwrap_or("off");
        let pending = self.cloud_sync_status["pending"].as_u64().unwrap_or(0);
        let label = match code {
            "off" => "sync.off",
            "waiting" => "sync.waiting",
            "syncing" => "sync.syncing",
            "ready" if pending > 0 => "sync.waiting",
            "ready" => "sync.ready",
            "offline" => "sync.offline",
            "quota" => "sync.quota",
            "account_unavailable" => "sync.account",
            "configuration" => "sync.configuration",
            "upgrade_required" => "sync.upgrade_required",
            "conflict" => "sync.conflict",
            _ => "sync.error",
        };
        let mut group = form_group(cx).child(
            div()
                .flex()
                .items_center()
                .gap_4()
                .p_5()
                .child(div().flex_1().child(setting_label(
                    t!("sync.enabled").to_string(),
                    t!("sync.description").to_string(),
                    cx,
                )))
                .child(
                    div().debug_selector(|| "cloud-sync-toggle".into()).child(
                        Switch::new("cloud-sync-enabled")
                            .checked(enabled)
                            .on_click(cx.listener(|this, checked, _, cx| {
                                this.update_cloud_sync(Command::Enable { enabled: *checked }, cx);
                            })),
                    ),
                ),
        );
        let colors = cx.theme().colors;
        let healthy = enabled && code == "ready" && pending == 0;
        let issue = enabled && !matches!(code, "off" | "ready" | "waiting" | "syncing" | "offline");
        let tone = if healthy {
            colors.success
        } else if code == "syncing" {
            colors.accent
        } else if enabled {
            colors.warning
        } else {
            colors.muted_foreground
        };
        let badge = div()
            .flex()
            .items_center()
            .gap_1()
            .h(px(24.))
            .px_2()
            .rounded_full()
            .text_size(px(11.))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(tone)
            .bg(tone.opacity(0.10))
            .when(healthy, |badge| {
                badge.child(icon_16("check").size(px(12.)).text_color(tone))
            })
            .child(t!(if issue { "sync.attention" } else { label }).to_string());
        let last = self.cloud_sync_status["last_success"]
            .as_u64()
            .and_then(|last| local_clock_time(last, &chrono::Local));
        group = group.child(Divider::horizontal()).child(
            div()
                .flex()
                .items_center()
                .gap_4()
                .p_5()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap_2()
                                .child(badge)
                                .when(enabled && pending > 0, |row| {
                                    row.child(
                                        sidebar_tag(
                                            t!("sync.pending", count = pending).to_string(),
                                            false,
                                            cx,
                                        )
                                        .h(px(24.))
                                        .px_2()
                                        .text_size(px(11.)),
                                    )
                                }),
                        )
                        .when(issue, |view| {
                            view.child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(colors.muted_foreground)
                                    .child(t!(label).to_string()),
                            )
                        })
                        .when_some(last, |view, time| {
                            view.child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(colors.muted_foreground)
                                    .child(t!("sync.last", time = time).to_string()),
                            )
                        }),
                )
                .when(enabled, |view| {
                    view.child(
                        Button::new("cloud-sync-retry")
                            .label(t!("sync.retry").to_string())
                            .outline()
                            .h(px(32.))
                            .w(px(112.))
                            .flex_shrink_0()
                            .disabled(code == "syncing")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update_cloud_sync(Command::Retry, cx)
                            })),
                    )
                }),
        );
        if enabled && let Some(conflicts) = self.cloud_sync_status["conflicts"].as_array() {
            for (index, conflict) in conflicts.iter().enumerate() {
                let Some(id) = conflict["id"].as_str() else {
                    continue;
                };
                let local_id = id.to_string();
                let remote_id = id.to_string();
                let describe = |value: &serde_json::Value| {
                    if value.is_null() {
                        return t!("sync.deleted").to_string();
                    }
                    let text = |key: &str| value[key].as_str().unwrap_or("");
                    let host = text("host");
                    let port = value["port"].as_u64().unwrap_or(0);
                    let address = if port == 0 {
                        host.to_owned()
                    } else if host.contains(':') {
                        format!("[{host}]:{port}")
                    } else {
                        format!("{host}:{port}")
                    };
                    let mut parts = vec![format!(
                        "{}\n{}: {}",
                        if text("name").is_empty() {
                            host
                        } else {
                            text("name")
                        },
                        text("protocol").to_uppercase(),
                        address
                    )];
                    if !text("username").is_empty() {
                        parts.push(if text("domain").is_empty() {
                            text("username").to_owned()
                        } else {
                            format!("{}/{}", text("domain"), text("username"))
                        });
                    }
                    if let Some(relay) = value["relay"].as_object() {
                        let field =
                            |key: &str| relay.get(key).and_then(|v| v.as_str()).unwrap_or("");
                        parts.push(format!(
                            "{}: {} ({})",
                            t!("sync.relay"),
                            field("endpoint"),
                            field("transport").to_uppercase()
                        ));
                        for (key, label) in [
                            ("server_fingerprint", "sync.relay_fingerprint"),
                            ("host_fingerprint", "sync.host_fingerprint"),
                        ] {
                            let pin = field(key);
                            if !pin.is_empty() {
                                let short = if pin.len() > 24 && pin.is_ascii() {
                                    format!("{}…{}", &pin[..12], &pin[pin.len() - 8..])
                                } else {
                                    pin.to_owned()
                                };
                                parts.push(format!("{}: {}", t!(label), short));
                            }
                        }
                    }
                    parts.join("\n")
                };
                group = group.child(Divider::horizontal()).child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .p_5()
                        .child(
                            t!("sync.local_version", value = describe(&conflict["local"]))
                                .to_string(),
                        )
                        .child(
                            t!("sync.cloud_version", value = describe(&conflict["remote"]))
                                .to_string(),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_3()
                                .child(
                                    Button::new(("sync-keep-local", index))
                                        .label(t!("sync.keep_local").to_string())
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.update_cloud_sync(
                                                Command::Resolve {
                                                    id: local_id.clone(),
                                                    keep_local: true,
                                                },
                                                cx,
                                            )
                                        })),
                                )
                                .child(
                                    Button::new(("sync-use-cloud", index))
                                        .label(t!("sync.use_cloud").to_string())
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.update_cloud_sync(
                                                Command::Resolve {
                                                    id: remote_id.clone(),
                                                    keep_local: false,
                                                },
                                                cx,
                                            )
                                        })),
                                ),
                        ),
                );
            }
        }
        div()
            .flex()
            .flex_col()
            .gap_5()
            .w_full()
            .child(section_header(
                "cloud",
                t!("sync.title").to_string(),
                t!("sync.subtitle").to_string(),
                cx,
            ))
            .child(group)
    }
}
