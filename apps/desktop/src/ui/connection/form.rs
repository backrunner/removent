use super::*;

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

    pub(super) fn choose(
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

    /// Open an advertised endpoint with fresh per-connection credentials.
    pub fn prefill_discovered(
        &mut self,
        protocol: ConnectionProtocol,
        name: String,
        addr: std::net::SocketAddr,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.obscured || self.connecting {
            return;
        }
        self.saved_id = None;
        self.choose(protocol, window, cx);
        let address = ConnectionAddress::from_socket(addr);
        self.name.update(cx, |s, cx| s.set_value(name, window, cx));
        self.host
            .update(cx, |s, cx| s.set_value(address.host, window, cx));
        self.port.update(cx, |s, cx| {
            s.set_value(address.port.to_string(), window, cx)
        });
        window.focus(&self.username.focus_handle(cx));
    }

    /// Reopen a bookmark. `None` leaves its password empty for the user to fill.
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

    pub(super) fn relay_scope(&self, cx: &gpui::App) -> (String, String, String, RelayTransport) {
        (
            self.relay_endpoint.read(cx).value().to_string(),
            self.relay_pin.read(cx).value().to_string(),
            self.host.read(cx).value().to_string(),
            self.relay_transport,
        )
    }

    pub(super) fn choose_relay_transport(
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

    pub(super) fn select_relay(
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

    pub(super) fn save(&mut self, cx: &mut Context<Self>) {
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

    pub(super) fn submit(&mut self, cx: &mut Context<Self>) {
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

    pub(super) fn cycle_focus(&self, backwards: bool, window: &mut Window, cx: &gpui::App) {
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
