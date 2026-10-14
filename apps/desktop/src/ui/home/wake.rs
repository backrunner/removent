use super::*;
use gpui::{MouseButton, WeakEntity};
use gpui_component::{
    WindowExt,
    menu::{PopupMenu, PopupMenuItem},
};
use removent_client::wake_on_lan::{WakeError, WakeOnLan};

fn wake_error(error: &WakeError) -> String {
    match error {
        WakeError::MacAddress => t!("wol.invalid_mac").to_string(),
        WakeError::Destination => t!("wol.invalid_destination").to_string(),
        WakeError::Port => t!("wol.invalid_port").to_string(),
        WakeError::Send(error) => t!("wol.send_failed", err = error.to_string()).to_string(),
    }
}

impl HomeView {
    /// Used by both a row's context menu and the accessible More menu.
    pub(super) fn wake_menu(
        menu: PopupMenu,
        target: SavedConnection,
        home: WeakEntity<Self>,
    ) -> PopupMenu {
        let configured = target.wake_on_lan.is_some();
        let wake_target = target.clone();
        let wake_home = home.clone();
        menu.item(
            PopupMenuItem::new(t!("wol.send").to_string())
                .icon(icon_16("power"))
                .on_click(move |_, window, cx| {
                    let _ = wake_home.update(cx, |this, cx| {
                        if configured {
                            this.send_wake(&wake_target.id, cx);
                        } else {
                            this.open_wake_settings(wake_target.clone(), window, cx);
                        }
                    });
                }),
        )
        .item(
            PopupMenuItem::new(t!("wol.configure").to_string())
                .icon(icon_16("settings"))
                .on_click(move |_, window, cx| {
                    let _ = home.update(cx, |this, cx| {
                        this.open_wake_settings(target.clone(), window, cx);
                    });
                }),
        )
    }

    pub(super) fn discovered_wake_target(&self, row: &DeviceRow) -> SavedConnection {
        self.saved_discovery_match(row)
            .cloned()
            .unwrap_or_else(|| SavedConnection {
                name: row.name.clone(),
                protocol: row.protocol,
                host: removent_client::connection::ConnectionAddress::from_socket(row.addr).host,
                port: row.addr.port(),
                credentials_review_required: row.protocol != ConnectionProtocol::Removent,
                ..Default::default()
            })
    }

    pub(super) fn send_wake(&mut self, id: &str, cx: &mut Context<Self>) {
        // Read on the worker: file locks and socket operations never block GPUI.
        let engine = self.engine.clone();
        let id = id.to_owned();
        self.set_status(t!("wol.sending").to_string(), StatusTone::Info);
        let send = cx.background_executor().spawn(async move {
            let entry = engine.saved_connections().into_iter().find(|e| e.id == id);
            match entry.and_then(|entry| entry.wake_on_lan) {
                Some(config) => config.send().map_err(|error| wake_error(&error)),
                None => Err(t!("wol.missing_config").to_string()),
            }
        });
        cx.spawn(async move |this, cx| {
            let result = send.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => this.set_status(t!("wol.sent").to_string(), StatusTone::Ok),
                    Err(error) => this.set_status(error, StatusTone::Err),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn open_wake_settings(
        &mut self,
        target: SavedConnection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.connection_dialog.is_some()
            || self.pin_dialog.is_some()
            || self.admission.is_some()
            || window.has_active_dialog(cx)
        {
            return;
        }
        // A discovered RDP endpoint has no account yet; its bookmark must first
        // pass the ordinary connection form's account validation (including sync).
        if target.id.is_empty() && target.protocol == ConnectionProtocol::Rdp {
            self.open_connection_dialog(None, window, cx);
            if let Some(dialog) = &self.connection_dialog
                && let Ok(address) = target.address().to_string().parse::<SocketAddr>()
            {
                dialog.update(cx, |form, cx| {
                    form.prefill_discovered(target.protocol, target.name, address, window, cx);
                });
            }
            self.set_status(t!("wol.save_first").to_string(), StatusTone::Info);
            cx.notify();
            return;
        }
        let home = cx.weak_entity();
        let form = cx.new(|cx| WakeSettings::new(target, self.engine.clone(), home, window, cx));
        window.open_dialog(cx, move |dialog, _, cx| {
            let busy = form.read(cx).busy;
            let confirm = form.clone();
            let cancel = form.clone();
            dialog
                .title(t!("wol.title").to_string())
                .width(px(440.))
                .close_button(!busy)
                .keyboard(!busy)
                .overlay_closable(false)
                .on_ok(move |_, window, cx| {
                    confirm.update(cx, |form, cx| form.save(false, window, cx));
                    // Async commit owns closing; validation and write failures
                    // must retain the form, including confirmation via Enter.
                    false
                })
                .on_cancel(move |_, _, cx| !cancel.read(cx).busy)
                .child(form.clone())
        });
    }
}

struct WakeSettings {
    target: SavedConnection,
    engine: Engine,
    home: WeakEntity<HomeView>,
    mac: Entity<InputState>,
    destination: Entity<InputState>,
    port: Entity<InputState>,
    error: Option<String>,
    busy: bool,
    _subscriptions: Vec<gpui::Subscription>,
}

impl WakeSettings {
    fn new(
        target: SavedConnection,
        engine: Engine,
        home: WeakEntity<HomeView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let config = target.wake_on_lan.as_ref();
        let mac = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("02:11:22:33:44:55");
            input.set_value(
                config.map(|c| c.mac_address.clone()).unwrap_or_default(),
                window,
                cx,
            );
            input
        });
        let destination = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("192.168.1.255");
            input.set_value(
                config
                    .map(|c| c.destination.to_string())
                    .unwrap_or_else(|| "255.255.255.255".into()),
                window,
                cx,
            );
            input
        });
        let port = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_value(
                config
                    .map(|c| c.port.to_string())
                    .unwrap_or_else(|| "9".into()),
                window,
                cx,
            );
            input
        });
        let mut subscriptions = Vec::new();
        for input in [&mac, &destination, &port] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => this.save(false, window, cx),
                    InputEvent::Change => {
                        this.error = None;
                        cx.notify();
                    }
                    _ => {}
                },
            ));
        }
        Self {
            target,
            engine,
            home,
            mac,
            destination,
            port,
            error: None,
            busy: false,
            _subscriptions: subscriptions,
        }
    }

    fn save(&mut self, clear: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let config = if clear {
            None
        } else {
            match WakeOnLan::parse(
                &self.mac.read(cx).value(),
                &self.destination.read(cx).value(),
                &self.port.read(cx).value(),
            ) {
                Ok(config) => Some(config),
                Err(error) => {
                    self.error = Some(wake_error(&error));
                    cx.notify();
                    return;
                }
            }
        };
        self.busy = true;
        self.error = None;
        let engine = self.engine.clone();
        let target = self.target.clone();
        let save = cx.background_executor().spawn(async move {
            let result = engine.set_wake_on_lan(&target, config);
            (result, engine.saved_connections())
        });
        cx.spawn_in(window, async move |this, cx| {
            let (result, saved) = save.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(entry) => {
                        let _ = this.home.update(cx, |home, cx| {
                            home.saved = saved;
                            home.selected = Some(Selection::Saved(entry.id));
                            home.settings_open = false;
                            home.set_status(
                                t!(if clear { "wol.cleared" } else { "wol.saved" }).to_string(),
                                StatusTone::Ok,
                            );
                            cx.notify();
                        });
                        window.close_dialog(cx);
                    }
                    Err(error) => {
                        this.error =
                            Some(t!("wol.save_failed", err = error.to_string()).to_string())
                    }
                }
                cx.notify();
                window.refresh();
            });
        })
        .detach();
        cx.notify();
        window.refresh();
    }
}

impl Render for WakeSettings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let field = |key: &'static str, selector: &'static str, input: &Entity<InputState>| {
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(section_label(t!(key).to_string(), cx))
                .child(
                    div()
                        .debug_selector(move || selector.into())
                        .child(form_input(input).disabled(self.busy)),
                )
        };
        div()
            .flex()
            .flex_col()
            .gap_4()
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_size(px(13.))
                    .debug_selector({
                        let id = self.target.id.clone();
                        move || format!("wol-target-{id}")
                    })
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(self.target.display_name()),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(colors.muted_foreground)
                    .child(t!("wol.hint").to_string()),
            )
            .child(field("wol.mac", "wol-mac", &self.mac))
            .child(field(
                "wol.destination",
                "wol-destination",
                &self.destination,
            ))
            .child(field("wol.port", "wol-port", &self.port))
            .when_some(self.error.clone(), |el, error| {
                el.child(
                    div()
                        .debug_selector(|| "wol-error".into())
                        .text_size(px(12.))
                        .text_color(colors.danger)
                        .child(error),
                )
            })
            .child(
                div()
                    .debug_selector(|| "wol-footer".into())
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        Button::new("wol-clear")
                            .label(t!("wol.clear").to_string())
                            .ghost()
                            .disabled(self.busy || self.target.wake_on_lan.is_none())
                            .on_click(
                                cx.listener(|this, _, window, cx| this.save(true, window, cx)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("wol-cancel")
                                    .label(t!("action.cancel").to_string())
                                    .outline()
                                    .disabled(self.busy)
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                            )
                            .child(
                                div().debug_selector(|| "wol-save-button".into()).child(
                                    Button::new("wol-save")
                                        .label(t!("action.save").to_string())
                                        .primary()
                                        .disabled(self.busy)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.save(false, window, cx)
                                        })),
                                ),
                            ),
                    ),
            )
    }
}

#[cfg(test)]
mod tests;
