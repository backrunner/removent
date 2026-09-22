//! Two-level connection dialog: protocol, then session-specific connection details.

use gpui::{
    Context, Entity, EventEmitter, FocusHandle, Focusable, Render, Window, div, prelude::*, px,
};
use gpui_component::{
    ActiveTheme, Disableable, Sizable,
    input::{InputEvent, InputState},
    spinner::Spinner,
    switch::Switch,
};
use removent_client::connection::{
    AddressError, ConnectionAddress, ConnectionProtocol, ConnectionRequest, ConnectionStage,
    RelayRoute, RelayTransport,
};
use removent_client::saved::SavedConnection;
use rust_i18n::t;
use std::time::{Duration, Instant};

use super::widgets::{Button, form_input, icon_16, section_header};

gpui::actions!(connection, [ConnectionTab, ConnectionTabPrev]);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionDialogEvent {
    Submit,
    Save,
    Cancel,
    Close,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Step {
    #[default]
    Protocol,
    Details(ConnectionProtocol),
}

pub struct ConnectionDialog {
    step: Step,
    saved_id: Option<String>,
    save_warning: Option<String>,
    name: Entity<InputState>,
    host: Entity<InputState>,
    port: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    domain: Entity<InputState>,
    accept_invalid_certificate: bool,
    via_relay: bool,
    relay_endpoint: Entity<InputState>,
    relay_transport: RelayTransport,
    relay_pin: Entity<InputState>,
    host_pin: Entity<InputState>,
    relay_choices: Vec<SavedConnection>,
    relay_secret_scope: Option<(String, String, String, RelayTransport)>,
    relay_secret_task: Option<gpui::Task<()>>,
    connecting: bool,
    stage: ConnectionStage,
    started: Option<Instant>,
    elapsed_task: Option<gpui::Task<()>>,
    retry: bool,
    cancelled: bool,
    obscured: bool,
    restore_focus: Option<FocusHandle>,
    error: Option<String>,
    focus: FocusHandle,
    _subscriptions: Vec<gpui::Subscription>,
}

impl EventEmitter<ConnectionDialogEvent> for ConnectionDialog {}

impl Focusable for ConnectionDialog {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ConnectionDialog {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let host = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("connection.host_placeholder").to_string())
        });
        let port = cx.new(|cx| InputState::new(window, cx));
        let username = cx.new(|cx| InputState::new(window, cx));
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));
        let domain = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("connection.optional").to_string())
        });
        let name = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("connection.name_placeholder").to_string())
        });
        let relay_endpoint = cx
            .new(|cx| InputState::new(window, cx).placeholder("removent://relay.example.com:443"));
        let relay_pin = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("connection.quic_pin_placeholder").to_string())
        });
        let host_pin = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("connection.fingerprint_placeholder").to_string())
        });
        let mut subscriptions = Vec::new();
        for input in [
            &name,
            &host,
            &port,
            &username,
            &password,
            &domain,
            &relay_endpoint,
            &relay_pin,
            &host_pin,
        ] {
            subscriptions.push(
                cx.subscribe(input, |this, _, ev: &InputEvent, cx| match ev {
                    InputEvent::PressEnter { .. } => this.submit(cx),
                    InputEvent::Change => {
                        this.error = None;
                        cx.notify();
                    }
                    _ => {}
                }),
            );
        }
        // A credential must never follow a changed destination or room silently.
        for input in [&relay_endpoint, &relay_pin, &host] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change)
                        && this.via_relay
                        && this
                            .relay_secret_scope
                            .as_ref()
                            .is_some_and(|scope| *scope != this.relay_scope(cx))
                    {
                        this.relay_secret_task = None;
                        this.password
                            .update(cx, |s, cx| s.set_value("", window, cx));
                    }
                },
            ));
        }
        subscriptions.push(cx.subscribe(&password, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) && this.via_relay {
                this.relay_secret_scope = Some(this.relay_scope(cx));
            }
        }));
        let focus = cx.focus_handle();
        window.focus(&focus);
        Self {
            step: Step::Protocol,
            saved_id: None,
            save_warning: None,
            name,
            host,
            port,
            username,
            password,
            domain,
            accept_invalid_certificate: false,
            via_relay: false,
            relay_endpoint,
            relay_transport: RelayTransport::default(),
            relay_pin,
            host_pin,
            relay_choices: Vec::new(),
            relay_secret_scope: None,
            relay_secret_task: None,
            connecting: false,
            stage: ConnectionStage::Resolving,
            started: None,
            elapsed_task: None,
            retry: false,
            cancelled: false,
            obscured: false,
            restore_focus: None,
            error: None,
            focus,
            _subscriptions: subscriptions,
        }
    }

    fn choose(
        &mut self,
        protocol: ConnectionProtocol,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.obscured || self.connecting {
            return;
        }
        self.step = Step::Details(protocol);
        self.via_relay = false;
        self.relay_secret_task = None;
        self.host.update(cx, |s, cx| {
            s.set_placeholder(t!("connection.host_placeholder").to_string(), window, cx)
        });
        self.error = None;
        self.retry = false;
        self.cancelled = false;
        self.accept_invalid_certificate = false;
        self.port.update(cx, |s, cx| {
            s.set_value(protocol.default_port().to_string(), window, cx)
        });
        // Switching protocols must never carry credentials to a different kind of server.
        for input in [&self.username, &self.password, &self.domain] {
            input.update(cx, |s, cx| s.set_value("", window, cx));
        }
        self.host.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    pub fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.obscured {
            return;
        }
        if self.connecting {
            cx.emit(ConnectionDialogEvent::Cancel);
        } else if self.step == Step::Protocol {
            cx.emit(ConnectionDialogEvent::Close);
        } else {
            self.step = Step::Protocol;
            self.error = None;
            self.password
                .update(cx, |s, cx| s.set_value("", window, cx));
            window.focus(&self.focus);
            cx.notify();
        }
    }

    pub fn set_connecting(
        &mut self,
        connecting: bool,
        error: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if connecting && !self.connecting {
            self.stage = ConnectionStage::Resolving;
            self.started = Some(Instant::now());
            self.cancelled = false;
            self.elapsed_task = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    if !this
                        .update(cx, |this, cx| {
                            cx.notify();
                            this.connecting
                        })
                        .unwrap_or(false)
                    {
                        break;
                    }
                }
            }));
        } else if !connecting {
            self.elapsed_task = None;
            self.started = None;
            self.retry = error.is_some();
        }
        self.connecting = connecting;
        self.error = error;
        cx.notify();
    }

    pub fn set_stage(&mut self, stage: ConnectionStage, cx: &mut Context<Self>) {
        if self.connecting {
            self.stage = stage;
            cx.notify();
        }
    }

    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        self.set_connecting(false, None, cx);
        self.cancelled = true;
    }

    /// A host-side PIN/admission dialog can temporarily cover this form. Remove
    /// its keyboard focus while covered and restore the exact field afterwards.
    pub fn set_obscured(
        &mut self,
        obscured: bool,
        fallback: &FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.obscured == obscured {
            return;
        }
        self.obscured = obscured;
        if obscured {
            // A protocol selection may have focused a field before GPUI has
            // rendered it into the focus tree for the first time.
            let field_focused = [
                &self.name,
                &self.host,
                &self.port,
                &self.username,
                &self.password,
                &self.domain,
                &self.relay_endpoint,
                &self.relay_pin,
                &self.host_pin,
            ]
            .into_iter()
            .any(|input| input.focus_handle(cx).is_focused(window));
            if self.focus.contains_focused(window, cx) || field_focused {
                self.restore_focus = window.focused(cx);
                window.focus(fallback);
            }
        } else {
            window.focus(
                &self
                    .restore_focus
                    .take()
                    .unwrap_or_else(|| self.focus.clone()),
            );
        }
        cx.notify();
    }

    pub fn request(&self, cx: &gpui::App) -> Result<ConnectionRequest, String> {
        let Step::Details(protocol) = self.step else {
            return Err(String::new());
        };
        let via_relay = protocol == ConnectionProtocol::Removent && self.via_relay;
        if via_relay && self.relay_secret_task.is_some() {
            return Err(t!("connection.relay_credential_loading").to_string());
        }
        let port = if via_relay {
            protocol.default_port().to_string()
        } else {
            self.port.read(cx).value().to_string()
        };
        let host = self.host.read(cx).value();
        let address = (if via_relay {
            ConnectionAddress::relay_room(&host)
        } else if protocol == ConnectionProtocol::Removent {
            ConnectionAddress::parse_native(&host, &port)
        } else {
            ConnectionAddress::parse(&host, &port)
        })
        .map_err(|e| {
            t!(match e {
                AddressError::Host => "connection.invalid_host",
                AddressError::Port => "connection.invalid_port",
            })
            .to_string()
        })?;
        let username = self.username.read(cx).value().trim().to_string();
        if protocol == ConnectionProtocol::Rdp && username.is_empty() {
            return Err(t!("connection.username_required").to_string());
        }
        let relay = if via_relay {
            let route = RelayRoute::parse(
                &self.relay_endpoint.read(cx).value(),
                self.relay_transport,
                &self.relay_pin.read(cx).value(),
                &self.host_pin.read(cx).value(),
            )
            .map_err(|key| t!(key).to_string())?;
            let token = self.password.read(cx).value();
            if !token.is_empty()
                && (token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                return Err(t!("connection.invalid_relay_credential").to_string());
            }
            Some(route)
        } else {
            None
        };
        Ok(ConnectionRequest {
            protocol,
            address,
            username,
            password: if protocol == ConnectionProtocol::Removent && !via_relay {
                String::new()
            } else {
                self.password.read(cx).value().to_string()
            },
            domain: self.domain.read(cx).value().trim().to_string(),
            accept_invalid_certificate: self.accept_invalid_certificate,
            relay,
        })
    }

    /// The optional memo name saved alongside the request when it is submitted.
    pub fn memo_name(&self, cx: &gpui::App) -> String {
        self.name.read(cx).value().trim().to_string()
    }

    /// Reopen the Details step with a saved connection's fields. `password` is
    /// whatever the Keychain still holds for the bookmark — `None` leaves the
    /// field empty for the user to fill.
    pub fn prefill(
        &mut self,
        saved: &SavedConnection,
        password: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.obscured || self.connecting {
            return;
        }
        self.saved_id = Some(saved.id.clone());
        self.via_relay = saved.relay.is_some();
        self.relay_secret_task = None;
        self.host.update(cx, |s, cx| {
            s.set_placeholder(
                if self.via_relay {
                    "office".to_string()
                } else {
                    t!("connection.host_placeholder").to_string()
                },
                window,
                cx,
            )
        });
        self.step = Step::Details(saved.protocol);
        self.error = None;
        self.retry = false;
        self.cancelled = false;
        self.accept_invalid_certificate = saved.accept_invalid_certificate;
        let port = if self.via_relay {
            saved.protocol.default_port()
        } else {
            saved.port
        }
        .to_string();
        let password = password.unwrap_or_default();
        let host = saved.host.clone();
        self.relay_transport = saved
            .relay
            .as_ref()
            .map(|r| r.transport)
            .unwrap_or_default();
        let endpoint = saved
            .relay
            .as_ref()
            .map(|r| r.endpoint.clone())
            .unwrap_or_default();
        let relay_pin = saved
            .relay
            .as_ref()
            .map(|r| r.server_fingerprint.clone())
            .unwrap_or_default();
        let host_pin = saved
            .relay
            .as_ref()
            .map(|r| r.host_fingerprint.clone())
            .unwrap_or_default();
        for (input, value) in [
            (&self.name, &saved.name),
            (&self.host, &host),
            (&self.port, &port),
            (&self.username, &saved.username),
            (&self.domain, &saved.domain),
            (&self.relay_endpoint, &endpoint),
            (&self.relay_pin, &relay_pin),
            (&self.host_pin, &host_pin),
            (&self.password, &password),
        ] {
            input.update(cx, |s, cx| s.set_value(value.clone(), window, cx));
        }
        self.relay_secret_scope = Some(self.relay_scope(cx));
        // A missing stored password is the only field the user must supply.
        if saved.protocol == ConnectionProtocol::Removent || !password.is_empty() {
            self.name.update(cx, |s, cx| s.focus(window, cx));
        } else {
            self.password.update(cx, |s, cx| s.focus(window, cx));
        }
        cx.notify();
    }

    fn relay_scope(&self, cx: &gpui::App) -> (String, String, String, RelayTransport) {
        (
            self.relay_endpoint.read(cx).value().to_string(),
            self.relay_pin.read(cx).value().to_string(),
            self.host.read(cx).value().to_string(),
            self.relay_transport,
        )
    }

    fn choose_relay_transport(
        &mut self,
        transport: RelayTransport,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.obscured || self.connecting || self.relay_transport == transport {
            return;
        }
        self.relay_transport = transport;
        self.relay_secret_task = None;
        self.password
            .update(cx, |s, cx| s.set_value("", window, cx));
        self.relay_pin
            .update(cx, |s, cx| s.set_value("", window, cx));
        self.relay_secret_scope = Some(self.relay_scope(cx));
        self.error = None;
        cx.notify();
    }

    pub fn set_relay_choices(&mut self, saved: Vec<SavedConnection>) {
        self.relay_choices = saved.into_iter().filter(|s| s.relay.is_some()).collect();
    }

    fn select_relay(
        &mut self,
        saved: &SavedConnection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(route) = &saved.relay else {
            return;
        };
        self.relay_secret_task = None;
        self.relay_transport = route.transport;
        self.password
            .update(cx, |s, cx| s.set_value("", window, cx));
        self.relay_endpoint
            .update(cx, |s, cx| s.set_value(route.endpoint.clone(), window, cx));
        self.relay_pin.update(cx, |s, cx| {
            s.set_value(route.server_fingerprint.clone(), window, cx)
        });
        // Read Keychain off the UI thread; a late result cannot follow an edit.
        if self.host.read(cx).value().trim() == saved.host
            && let Some(account) = saved.credential_account()
        {
            let account = account.to_owned();
            let scope = self.relay_scope(cx);
            let load = cx
                .background_executor()
                .spawn(async move { removent_client::keychain::load(&account) });
            self.relay_secret_task = Some(cx.spawn_in(window, async move |this, cx| {
                let result = load.await;
                let _ = this.update_in(cx, |this, window, cx| {
                    this.relay_secret_task = None;
                    if !this.via_relay || this.relay_scope(cx) != scope {
                        return;
                    }
                    match result {
                        Ok(Some(secret)) => this
                            .password
                            .update(cx, |s, cx| s.set_value(secret, window, cx)),
                        _ => {
                            this.error = Some(t!("connection.relay_credential_missing").to_string())
                        }
                    }
                    this.relay_secret_scope = Some(scope);
                    cx.notify();
                });
            }));
        }
        self.relay_secret_scope = Some(self.relay_scope(cx));
        cx.notify();
    }

    pub fn saved_id(&self) -> Option<&str> {
        self.saved_id.as_deref()
    }

    pub fn set_saved_id(&mut self, id: String) {
        self.saved_id = Some(id);
    }

    pub fn set_save_warning(&mut self, warning: Option<String>, cx: &mut Context<Self>) {
        self.save_warning = warning;
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if self.obscured || self.connecting {
            return;
        }
        match self.request(cx) {
            Ok(_) => cx.emit(ConnectionDialogEvent::Save),
            Err(error) => {
                self.error = Some(error);
                cx.notify();
            }
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.obscured || self.connecting || self.step == Step::Protocol {
            return;
        }
        match self.request(cx) {
            Ok(_) => {
                // Lock immediately: repeated Enter/click events must not queue attempts.
                self.set_connecting(true, None, cx);
                cx.emit(ConnectionDialogEvent::Submit);
            }
            Err(error) => {
                self.error = Some(error);
                cx.notify();
            }
        }
    }

    fn cycle_focus(&self, backwards: bool, window: &mut Window, cx: &gpui::App) {
        // Keep keyboard navigation inside the modal, including its buttons and toggles.
        for _ in 0..128 {
            if backwards {
                window.focus_prev();
            } else {
                window.focus_next();
            }
            if self.focus.contains_focused(window, cx) {
                break;
            }
        }
    }
}

impl Render for ConnectionDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let busy = self.connecting;
        let mut body = div().flex().flex_col().gap_4().p_6();
        match self.step {
            Step::Protocol => {
                body = body.child(
                    div()
                        .text_size(px(13.))
                        .text_color(colors.muted_foreground)
                        .child(t!("connection.subtitle").to_string()),
                );
                for (i, protocol) in ConnectionProtocol::ALL.into_iter().enumerate() {
                    let (name, description) = match protocol {
                        ConnectionProtocol::Removent => ("wifi", "connection.removent_hint"),
                        ConnectionProtocol::Vnc => ("monitor", "connection.vnc_hint"),
                        ConnectionProtocol::Rdp => ("copy", "connection.rdp_hint"),
                    };
                    body = body.child(
                        Button::new(("connection-protocol", i))
                            .outline()
                            .w_full()
                            .h(px(84.))
                            .rounded(px(14.))
                            .surface()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .w(px(388.))
                                    .max_w_full()
                                    .px_2()
                                    .child(
                                        section_header(
                                            name,
                                            protocol.label().into(),
                                            t!(description).to_string(),
                                            cx,
                                        )
                                        .flex_1(),
                                    )
                                    .child(
                                        icon_16("chevron-right")
                                            .text_color(colors.muted_foreground),
                                    ),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.choose(protocol, window, cx)
                            })),
                    );
                }
            }
            Step::Details(protocol) => {
                let field = |label: String, input: &Entity<InputState>| {
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .min_w_0()
                        .child(div().text_size(px(12.)).child(label))
                        .child(form_input(input).disabled(busy))
                };
                body = body.child(field(t!("connection.name").to_string(), &self.name));
                if protocol == ConnectionProtocol::Removent {
                    body = body.child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(t!("connection.use_relay").to_string())
                            .child(
                                Switch::new("connection-use-relay")
                                    .small()
                                    .checked(self.via_relay)
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.via_relay = !this.via_relay;
                                        this.relay_secret_task = None;
                                        this.host.update(cx, |s, cx| {
                                            s.set_placeholder(
                                                if this.via_relay {
                                                    "office".to_string()
                                                } else {
                                                    t!("connection.host_placeholder").to_string()
                                                },
                                                window,
                                                cx,
                                            )
                                        });
                                        this.password
                                            .update(cx, |s, cx| s.set_value("", window, cx));
                                        this.error = None;
                                        cx.notify();
                                    })),
                            ),
                    );
                }
                body = body.child(
                    div()
                        .flex()
                        .gap_3()
                        .child(
                            field(
                                t!(if self.via_relay {
                                    "connection.relay_room"
                                } else {
                                    "connection.address"
                                })
                                .to_string(),
                                &self.host,
                            )
                            .flex_1(),
                        )
                        .when(!self.via_relay, |el| {
                            el.child(
                                field(t!("connection.port").to_string(), &self.port)
                                    .w(px(88.))
                                    .flex_shrink_0(),
                            )
                        }),
                );
                if protocol != ConnectionProtocol::Removent {
                    body = body
                        .child(field(
                            t!(if protocol == ConnectionProtocol::Vnc {
                                "connection.vnc_username"
                            } else {
                                "connection.username"
                            })
                            .to_string(),
                            &self.username,
                        ))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .child(t!("connection.password").to_string()),
                                )
                                .child(form_input(&self.password).mask_toggle().disabled(busy)),
                        );
                }

                if protocol == ConnectionProtocol::Removent && self.via_relay {
                    body = body.child(
                        div()
                            .text_size(px(12.))
                            .child(t!("connection.relay_transport").to_string()),
                    );
                    let mut transports = div().flex().gap_2();
                    for (index, transport) in [RelayTransport::WebSocket, RelayTransport::Quic]
                        .into_iter()
                        .enumerate()
                    {
                        transports = transports.child(
                            Button::new(("relay-transport", index))
                                .outline()
                                .small()
                                .disabled(busy)
                                .label(
                                    t!(if transport == RelayTransport::WebSocket {
                                        "connection.relay_websocket"
                                    } else {
                                        "connection.relay_quic"
                                    })
                                    .to_string(),
                                )
                                .segment(self.relay_transport == transport)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.choose_relay_transport(transport, window, cx);
                                })),
                        );
                    }
                    body = body.child(transports);
                    for (index, saved) in self.relay_choices.clone().into_iter().enumerate() {
                        let route = saved.relay.as_ref().unwrap();
                        body = body.child(
                            Button::new(("saved-relay", index))
                                .outline()
                                .small()
                                .disabled(busy)
                                .label(format!("{} · {}", route.endpoint, saved.host))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.select_relay(&saved, window, cx)
                                })),
                        );
                    }
                    body = body
                        .child(field(
                            t!("connection.relay_endpoint").to_string(),
                            &self.relay_endpoint,
                        ))
                        .child(field(
                            t!("connection.relay_pin").to_string(),
                            &self.relay_pin,
                        ))
                        .child(field(
                            t!("connection.host_fingerprint").to_string(),
                            &self.host_pin,
                        ))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .child(t!("connection.relay_credential").to_string()),
                                )
                                .child(
                                    form_input(&self.password)
                                        .mask_toggle()
                                        .disabled(busy || self.relay_secret_task.is_some()),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(colors.muted_foreground)
                                .child(t!("connection.relay_trust_hint").to_string()),
                        );
                }
                if protocol == ConnectionProtocol::Rdp {
                    body =
                        body.child(field(t!("connection.domain").to_string(), &self.domain))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .child(
                                        div().flex_1().min_w_0().text_size(px(12.)).child(
                                            t!("connection.certificate_exception").to_string(),
                                        ),
                                    )
                                    .child(
                                        Switch::new("rdp-certificate-exception")
                                            .small()
                                            .checked(self.accept_invalid_certificate)
                                            .disabled(busy)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.accept_invalid_certificate =
                                                    !this.accept_invalid_certificate;
                                                cx.notify();
                                            })),
                                    ),
                            );
                }
                if protocol == ConnectionProtocol::Vnc {
                    body = body.child(
                        div()
                            .p_3()
                            .rounded(px(10.))
                            .bg(colors.accent.opacity(0.08))
                            .text_size(px(12.))
                            .text_color(colors.muted_foreground)
                            .child(t!("connection.credentials_hint").to_string()),
                    );
                }
            }
        }
        div()
            .id("connection-form")
            .track_focus(&self.focus)
            .tab_group()
            .tab_index(0)
            .tab_stop(false)
            .key_context("ConnectionDialog")
            // Single-line inputs emit PressEnter, then propagate their action. Consume
            // it here so GPUI does not insert a literal newline after submitting.
            .on_action(|_: &gpui_component::input::Enter, _, cx| cx.stop_propagation())
            .on_action(cx.listener(|this, _: &ConnectionTab, window, cx| {
                this.cycle_focus(false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ConnectionTabPrev, window, cx| {
                this.cycle_focus(true, window, cx)
            }))
            .max_h((window.viewport_size().height - px(64.)).max(px(200.)))
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .p_6()
                    .pb_4()
                    .border_b_1()
                    .border_color(colors.border)
                    .items_center()
                    .gap_2()
                    .when(matches!(self.step, Step::Details(_)), |el| {
                        el.child(
                            Button::new("connection-back")
                                .ghost()
                                .small()
                                .icon(icon_16("arrow-left"))
                                .tooltip(t!("action.back").to_string())
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, window, cx| this.back(window, cx))),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(20.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(match self.step {
                                Step::Protocol => t!("connection.add").to_string(),
                                Step::Details(p) => p.label().to_string(),
                            }),
                    )
                    .child(
                        Button::new("connection-close")
                            .ghost()
                            .small()
                            .icon(icon_16("x"))
                            .tooltip(
                                t!(if busy {
                                    "connection.cancel_and_close"
                                } else {
                                    "action.close"
                                })
                                .to_string(),
                            )
                            .on_click(
                                cx.listener(|_, _, _, cx| cx.emit(ConnectionDialogEvent::Close)),
                            ),
                    ),
            )
            .child(
                div()
                    .id("connection-fields")
                    .debug_selector(|| "connection-fields".into())
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body),
            )
            .when(matches!(self.step, Step::Details(_)), |el| {
                el.child(
                    div()
                        .debug_selector(|| "connection-footer".into())
                        .flex()
                        .flex_col()
                        .gap_3()
                        .flex_shrink_0()
                        .px_6()
                        .py_4()
                        .border_t_1()
                        .border_color(colors.border)
                        .when_some(self.error.clone(), |el, error| {
                            el.child(
                                div()
                                    .id("connection-error")
                                    .max_h(px(110.))
                                    .overflow_y_scroll()
                                    .p_3()
                                    .rounded(px(10.))
                                    .bg(colors.danger.opacity(0.08))
                                    .border_1()
                                    .border_color(colors.danger.opacity(0.18))
                                    .text_size(px(12.))
                                    .text_color(colors.danger)
                                    .child(error),
                            )
                        })
                        .when_some(self.save_warning.clone(), |el, warning| {
                            el.child(
                                div()
                                    .id("connection-save-warning")
                                    .max_h(px(90.))
                                    .overflow_y_scroll()
                                    .text_size(px(12.))
                                    .text_color(colors.warning)
                                    .child(warning),
                            )
                        })
                        .when(self.cancelled, |el| {
                            el.child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(colors.muted_foreground)
                                    .child(t!("connection.cancelled_hint").to_string()),
                            )
                        })
                        .when(busy, |el| {
                            let elapsed = self.started.map(|s| s.elapsed().as_secs()).unwrap_or(0);
                            el.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .child(Spinner::new().icon(icon_16("loader-circle")).small())
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_size(px(12.))
                                            .child(stage_label(self.stage)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(colors.muted_foreground)
                                            .child(
                                                t!("connection.elapsed", seconds = elapsed)
                                                    .to_string(),
                                            ),
                                    ),
                            )
                            .when(elapsed >= 8, |el| {
                                el.child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(colors.muted_foreground)
                                        .child(t!("connection.waiting_hint").to_string()),
                                )
                            })
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_end()
                                .gap_2()
                                .child(
                                    Button::new("cancel-connection")
                                        .outline()
                                        .label(
                                            t!(if busy {
                                                "connection.cancel_attempt"
                                            } else {
                                                "action.cancel"
                                            })
                                            .to_string(),
                                        )
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            cx.emit(if this.connecting {
                                                ConnectionDialogEvent::Cancel
                                            } else {
                                                ConnectionDialogEvent::Close
                                            });
                                        })),
                                )
                                .when(self.saved_id.is_some(), |el| {
                                    el.child(
                                        Button::new("save-connection")
                                            .outline()
                                            .label(t!("action.save").to_string())
                                            .disabled(busy)
                                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                                    )
                                })
                                .child(
                                    Button::new("submit-connection")
                                        .primary()
                                        .label(
                                            t!(if busy {
                                                "connection.connecting"
                                            } else if self.retry {
                                                "connection.retry"
                                            } else {
                                                "action.connect"
                                            })
                                            .to_string(),
                                        )
                                        .disabled(busy)
                                        .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                                ),
                        ),
                )
            })
    }
}

pub fn stage_label(stage: ConnectionStage) -> String {
    t!(match stage {
        ConnectionStage::Resolving => "connection.stage_resolving",
        ConnectionStage::Connecting => "connection.stage_connecting",
        ConnectionStage::Negotiating => "connection.stage_negotiating",
        ConnectionStage::Pairing => "connection.stage_pairing",
        ConnectionStage::Authenticating => "connection.stage_authenticating",
        ConnectionStage::PreparingDesktop => "connection.stage_desktop",
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AnyView, KeyBinding, TestAppContext};
    use gpui_component::Root;
    use std::cell::RefCell;
    use std::rc::Rc;

    struct TestSurface(Entity<ConnectionDialog>);
    impl Render for TestSurface {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(Button::new("background-control").label("Background"))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(div().w(px(480.)).child(self.0.clone())),
                )
        }
    }

    fn setup(cx: &mut TestAppContext) -> (Entity<ConnectionDialog>, &mut gpui::VisualTestContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.bind_keys([
                KeyBinding::new("tab", ConnectionTab, Some("ConnectionDialog")),
                KeyBinding::new("shift-tab", ConnectionTabPrev, Some("ConnectionDialog")),
            ]);
        });
        let slot = Rc::new(RefCell::new(None));
        let out = slot.clone();
        let (_, cx) = cx.add_window_view(|window, cx| {
            let dialog = cx.new(|cx| ConnectionDialog::new(window, cx));
            *out.borrow_mut() = Some(dialog.clone());
            let surface = cx.new(|_| TestSurface(dialog));
            Root::new(AnyView::from(surface), window, cx)
        });
        let dialog = slot.borrow_mut().take().unwrap();
        (dialog, cx)
    }

    fn relay_bookmark() -> SavedConnection {
        SavedConnection {
            id: "relay-bookmark".into(),
            protocol: ConnectionProtocol::Removent,
            host: "office".into(),
            port: 0,
            relay: Some(
                RelayRoute::parse(
                    "removent://relay.example:443",
                    RelayTransport::WebSocket,
                    "",
                    &"aa".repeat(32),
                )
                .unwrap(),
            ),
            password_hint: true,
            ..Default::default()
        }
    }

    #[gpui::test]
    fn switching_carrier_clears_credentials_and_requires_its_trust(cx: &mut TestAppContext) {
        let (dialog, cx) = setup(cx);
        cx.update(|window, cx| {
            dialog.update(cx, |form, cx| {
                form.prefill(&relay_bookmark(), Some("11".repeat(32)), window, cx);
            })
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            dialog.update(cx, |form, cx| {
                form.choose_relay_transport(RelayTransport::Quic, window, cx);
                assert!(form.password.read(cx).value().is_empty());
                assert!(form.request(cx).is_err(), "QUIC needs its own relay pin");
                form.relay_pin
                    .update(cx, |s, cx| s.set_value("bb".repeat(32), window, cx));
                let request = form.request(cx).unwrap();
                assert_eq!(request.relay.unwrap().transport, RelayTransport::Quic);
                form.password
                    .update(cx, |s, cx| s.set_value("22".repeat(32), window, cx));
                form.choose_relay_transport(RelayTransport::WebSocket, window, cx);
                assert!(form.password.read(cx).value().is_empty());
                let request = form.request(cx).unwrap();
                assert!(request.relay.unwrap().server_fingerprint.is_empty());
            })
        });
    }

    #[gpui::test]
    fn relay_prefill_preserves_secret_but_destination_edits_clear_it(cx: &mut TestAppContext) {
        let (dialog, cx) = setup(cx);
        cx.update(|window, cx| {
            dialog.update(cx, |form, cx| {
                form.prefill(&relay_bookmark(), Some("11".repeat(32)), window, cx)
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            let form = dialog.read(cx);
            assert!(form.via_relay);
            let request = form.request(cx).unwrap();
            assert_eq!(request.password, "11".repeat(32));
            assert_eq!(request.address.host, "office");
            assert_eq!(request.relay, relay_bookmark().relay);
        });
        cx.update(|window, cx| {
            dialog.update(cx, |form, cx| {
                form.relay_endpoint.update(cx, |s, cx| {
                    s.set_value("removent://another.example:443", window, cx)
                });
            })
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            dialog.update(cx, |form, cx| {
                assert!(form.password.read(cx).value().is_empty());
                assert!(
                    form.request(cx).unwrap().password.is_empty(),
                    "credential-free registration is valid"
                );
                form.password
                    .update(cx, |s, cx| s.set_value("22".repeat(32), window, cx));
            })
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            dialog.update(cx, |form, cx| {
                form.host
                    .update(cx, |s, cx| s.set_value("other-room", window, cx));
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| assert!(dialog.read(cx).password.read(cx).value().is_empty()));
    }

    #[gpui::test]
    fn relay_selection_does_not_reuse_a_previous_routes_credential(cx: &mut TestAppContext) {
        let (dialog, cx) = setup(cx);
        cx.update(|window, cx| {
            dialog.update(cx, |form, cx| {
                form.prefill(&relay_bookmark(), Some("11".repeat(32)), window, cx)
            })
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            dialog.update(cx, |form, cx| {
                let mut other = relay_bookmark();
                other.password_hint = false;
                other.relay.as_mut().unwrap().endpoint = "removent://second.example:443".into();
                form.select_relay(&other, window, cx);
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            let request = dialog.read(cx).request(cx).unwrap();
            assert!(request.password.is_empty());
            assert_eq!(
                request.relay.unwrap().endpoint,
                "removent://second.example:443"
            );
        });
    }

    #[gpui::test]
    fn relay_form_scrolls_and_keeps_actions_visible(cx: &mut TestAppContext) {
        let (dialog, cx) = setup(cx);
        cx.simulate_resize(gpui::size(px(860.), px(600.)));
        cx.update(|window, cx| {
            dialog.update(cx, |form, cx| {
                form.set_relay_choices(vec![relay_bookmark()]);
                form.prefill(&relay_bookmark(), None, window, cx);
                form.port.update(cx, |s, cx| s.set_value("0", window, cx));
                form.save(cx);
                assert!(
                    form.error.is_none(),
                    "relay routes must ignore the hidden direct port"
                );
            })
        });
        cx.run_until_parked();
        let fields = cx.debug_bounds("connection-fields").unwrap();
        let footer = cx.debug_bounds("connection-footer").unwrap();
        assert!(fields.size.height > px(100.));
        assert!(footer.top() >= fields.bottom());
        assert!(footer.bottom() <= px(600.));
        for _ in 0..20 {
            cx.simulate_keystrokes("tab");
            cx.update(|window, cx| assert!(dialog.read(cx).focus.contains_focused(window, cx)));
        }
    }

    #[gpui::test]
    fn switching_protocol_clears_credentials_and_uses_correct_port(cx: &mut TestAppContext) {
        let (dialog, cx) = setup(cx);
        cx.update(|window, cx| {
            dialog.update(cx, |form, cx| {
                form.choose(ConnectionProtocol::Rdp, window, cx);
                form.host
                    .update(cx, |s, cx| s.set_value("office.local", window, cx));
                form.username
                    .update(cx, |s, cx| s.set_value("alice", window, cx));
                form.password
                    .update(cx, |s, cx| s.set_value("private", window, cx));
                form.domain
                    .update(cx, |s, cx| s.set_value("office", window, cx));
                form.accept_invalid_certificate = true;
                assert_eq!(form.request(cx).unwrap().address.port, 3389);
                form.back(window, cx);
                assert!(form.password.read(cx).value().is_empty());
                form.choose(ConnectionProtocol::Vnc, window, cx);
                let request = form.request(cx).unwrap();
                assert_eq!(request.address.port, 5900);
                assert!(
                    request.username.is_empty()
                        && request.password.is_empty()
                        && request.domain.is_empty()
                );
                assert!(!request.accept_invalid_certificate);
            })
        });
    }

    #[gpui::test]
    fn enter_validates_and_failure_allows_editing_and_retry(cx: &mut TestAppContext) {
        let (dialog, cx) = setup(cx);
        cx.update(|window, cx| {
            dialog.update(cx, |form, cx| {
                form.choose(ConnectionProtocol::Rdp, window, cx);
            })
        });
        cx.simulate_keystrokes("enter");
        cx.update(|window, cx| {
            dialog.update(cx, |form, cx| {
                assert!(form.error.is_some());
                form.host
                    .update(cx, |s, cx| s.set_value("192.168.1.10", window, cx));
                assert!(form.request(cx).is_err()); // RDP username is required.
                form.username
                    .update(cx, |s, cx| s.set_value("alice", window, cx));
                form.port
                    .update(cx, |s, cx| s.set_value("3390", window, cx));
                assert_eq!(form.request(cx).unwrap().address.port, 3390);
                form.set_connecting(true, None, cx);
                form.set_connecting(false, Some("Authentication failed".into()), cx);
                assert_eq!(form.host.read(cx).value().as_str(), "192.168.1.10");
                assert!(form.request(cx).is_ok());
            })
        });
    }

    #[gpui::test]
    fn bookmark_edit_keeps_identity_and_can_save_without_connecting(cx: &mut TestAppContext) {
        let (dialog, cx) = setup(cx);
        let mut events = cx.events(&dialog);
        cx.update(|window, cx| {
            dialog.update(cx, |form, cx| {
                let saved = SavedConnection {
                    id: "stable-bookmark".into(),
                    protocol: ConnectionProtocol::Vnc,
                    host: "old.local".into(),
                    port: 5900,
                    ..Default::default()
                };
                form.prefill(&saved, None, window, cx);
                form.host
                    .update(cx, |s, cx| s.set_value("new.local", window, cx));
                form.save(cx);
                assert_eq!(form.saved_id(), Some("stable-bookmark"));
                assert!(!form.connecting);
                form.set_save_warning(Some("Keychain unavailable".into()), cx);
                form.set_connecting(true, None, cx);
                form.set_stage(ConnectionStage::PreparingDesktop, cx);
                assert_eq!(form.save_warning.as_deref(), Some("Keychain unavailable"));
            })
        });
        cx.run_until_parked();
        assert_eq!(events.try_recv().unwrap(), ConnectionDialogEvent::Save);
    }

    #[gpui::test]
    fn keyboard_focus_stays_in_the_dialog_at_minimum_window_size(cx: &mut TestAppContext) {
        let (dialog, cx) = setup(cx);
        cx.simulate_resize(gpui::size(px(860.), px(600.)));
        cx.update(|window, cx| {
            dialog.update(cx, |form, cx| {
                form.choose(ConnectionProtocol::Rdp, window, cx)
            })
        });
        for _ in 0..18 {
            cx.simulate_keystrokes("tab");
            cx.update(|window, cx| assert!(dialog.read(cx).focus.contains_focused(window, cx)));
        }
        for _ in 0..18 {
            cx.simulate_keystrokes("shift-tab");
            cx.update(|window, cx| assert!(dialog.read(cx).focus.contains_focused(window, cx)));
        }
    }

    #[gpui::test]
    fn connection_footer_stays_visible_at_minimum_size(cx: &mut TestAppContext) {
        let (dialog, cx) = setup(cx);
        cx.simulate_resize(gpui::size(px(860.), px(600.)));
        cx.update(|window, cx| {
            dialog.update(cx, |form, cx| {
                form.choose(ConnectionProtocol::Rdp, window, cx);
                form.set_connecting(true, None, cx);
            })
        });
        cx.run_until_parked();
        let fields = cx.debug_bounds("connection-fields").unwrap();
        let footer = cx.debug_bounds("connection-footer").unwrap();
        assert!(fields.size.height > px(100.));
        assert!(footer.top() >= fields.bottom());
        assert!(footer.bottom() <= px(600.));
        cx.update(|_, cx| dialog.update(cx, |form, cx| {
            form.set_connecting(false, Some("The remote device refused the connection. Check the address and try again.".into()), cx);
        }));
        cx.run_until_parked();
        let footer = cx.debug_bounds("connection-footer").unwrap();
        assert!(footer.bottom() <= px(600.));
    }

    #[gpui::test]
    fn back_returns_one_level_and_cancels_a_pending_connection(cx: &mut TestAppContext) {
        let (dialog, cx) = setup(cx);
        let mut events = cx.events(&dialog);
        cx.update(|window, cx| {
            dialog.update(cx, |form, cx| {
                form.choose(ConnectionProtocol::Rdp, window, cx);
                form.back(window, cx);
                assert!(matches!(form.step, Step::Protocol));
            })
        });
        assert!(events.try_recv().is_err());
        cx.update(|window, cx| dialog.update(cx, |form, cx| form.back(window, cx)));
        assert_eq!(events.try_recv().unwrap(), ConnectionDialogEvent::Close);
        cx.update(|window, cx| {
            dialog.update(cx, |form, cx| {
                form.choose(ConnectionProtocol::Rdp, window, cx);
                form.set_connecting(true, None, cx);
                form.back(window, cx);
            })
        });
        assert_eq!(events.try_recv().unwrap(), ConnectionDialogEvent::Cancel);
    }

    #[gpui::test]
    fn submit_is_guarded_and_cancel_preserves_details_for_retry(cx: &mut TestAppContext) {
        let (dialog, cx) = setup(cx);
        let mut events = cx.events(&dialog);
        cx.update(|window, cx| {
            dialog.update(cx, |form, cx| {
                form.choose(ConnectionProtocol::Vnc, window, cx);
                form.host
                    .update(cx, |s, cx| s.set_value("192.168.1.11", window, cx));
                form.password
                    .update(cx, |s, cx| s.set_value("secret", window, cx));
                form.submit(cx);
                form.submit(cx);
                assert!(form.connecting);
                assert!(form.elapsed_task.is_some());
                form.set_stage(ConnectionStage::Negotiating, cx);
                assert_eq!(form.stage, ConnectionStage::Negotiating);
            })
        });
        assert_eq!(events.try_recv().unwrap(), ConnectionDialogEvent::Submit);
        assert!(events.try_recv().is_err());
        cx.update(|_, cx| {
            dialog.update(cx, |form, cx| {
                form.cancel(cx);
                assert!(form.cancelled && !form.connecting);
                assert!(form.elapsed_task.is_none());
                assert_eq!(form.request(cx).unwrap().password, "secret");
                form.set_stage(ConnectionStage::PreparingDesktop, cx);
                assert_eq!(form.stage, ConnectionStage::Negotiating);
                form.submit(cx);
                assert!(form.connecting && !form.cancelled);
                assert_eq!(form.stage, ConnectionStage::Resolving);
                form.set_connecting(false, Some("Authentication failed".into()), cx);
                assert!(form.retry);
                assert!(form.elapsed_task.is_none());
            })
        });
        assert_eq!(events.try_recv().unwrap(), ConnectionDialogEvent::Submit);
    }

    #[gpui::test]
    fn covered_form_does_not_receive_keys_or_submit_and_restores_focus(cx: &mut TestAppContext) {
        let (dialog, cx) = setup(cx);
        let mut events = cx.events(&dialog);
        let fallback = cx.update(|window, cx| {
            let fallback = cx.focus_handle();
            dialog.update(cx, |form, cx| {
                form.choose(ConnectionProtocol::Vnc, window, cx);
                form.host
                    .update(cx, |s, cx| s.set_value("localhost", window, cx));
                form.set_obscured(true, &fallback, window, cx);
                form.submit(cx);
                form.back(window, cx);
                assert!(!form.focus.contains_focused(window, cx));
                assert!(matches!(form.step, Step::Details(ConnectionProtocol::Vnc)));
            });
            fallback
        });
        assert!(events.try_recv().is_err());
        cx.update(|window, cx| {
            dialog.update(cx, |form, cx| {
                form.set_obscured(false, &fallback, window, cx);
                assert!(form.host.focus_handle(cx).is_focused(window));
                assert_eq!(form.host.read(cx).value().as_str(), "localhost");
            })
        });
    }
}
