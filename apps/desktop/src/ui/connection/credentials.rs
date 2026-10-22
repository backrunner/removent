use super::*;
use crate::engine::{AuthenticationAnswer, Engine, NativeCredential, NativeCredentialEdit};
use removent_core::AuthenticationMode;

#[cfg(test)]
mod tests;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct CredentialScope {
    step: Step,
    host: String,
    port: String,
    via_relay: bool,
    relay: (String, String, String, RelayTransport, String, bool),
    host_pin: String,
}

pub(super) struct CredentialFields {
    original: NativeCredential,
    pub username: Entity<InputState>,
    pub password: Entity<InputState>,
    remember: bool,
}

impl ConnectionDialog {
    fn credential_scope(&self, cx: &gpui::App) -> CredentialScope {
        CredentialScope {
            step: self.step,
            host: self.host.read(cx).value().to_string(),
            port: self.port.read(cx).value().to_string(),
            via_relay: self.via_relay,
            relay: self.relay_scope(cx),
            host_pin: self.host_pin.clone(),
        }
    }

    pub(super) fn clear_native_credentials(&mut self) {
        self.native_credential_task = None;
        self.native_credential_scope = None;
        self.native_credentials.clear();
    }

    pub fn load_native_credentials(
        &mut self,
        engine: Engine,
        request: ConnectionRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if request.protocol != ConnectionProtocol::Removent
            || !crate::engine::can_remember(AuthenticationMode::Password)
        {
            return;
        }
        let scope = self.credential_scope(cx);
        let load = cx
            .background_executor()
            .spawn(async move { engine.native_credentials(&request) });
        self.native_credential_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = load.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.native_credential_task = None;
                if this.credential_scope(cx) == scope {
                    match result {
                        Ok(credentials) => this.set_native_credentials(credentials, window, cx),
                        Err(_) => {
                            this.save_warning = Some(t!("auth.keychain_edit_failed").to_string())
                        }
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn set_native_credentials(
        &mut self,
        credentials: Vec<NativeCredential>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.native_credential_scope = Some(self.credential_scope(cx));
        self.native_credentials = credentials
            .into_iter()
            .map(|original| {
                let (name, secret) = if original.mode == AuthenticationMode::SystemAccount {
                    removent_proto::SystemCredentials::decode(&original.answer.value)
                        .map(|value| (value.username, value.password))
                        .unwrap_or_default()
                } else {
                    (String::new(), original.answer.value.clone())
                };
                let username = cx.new(|cx| InputState::new(window, cx).default_value(name));
                let password = cx.new(|cx| {
                    InputState::new(window, cx)
                        .masked(true)
                        .default_value(secret)
                });
                for input in [&username, &password] {
                    self._subscriptions.push(cx.subscribe(
                        input,
                        |this, _, event: &InputEvent, cx| match event {
                            InputEvent::PressEnter { .. } => this.submit(cx),
                            InputEvent::Change => {
                                this.error = None;
                                cx.notify();
                            }
                            _ => {}
                        },
                    ));
                }
                CredentialFields {
                    original,
                    username,
                    password,
                    remember: true,
                }
            })
            .collect();
        cx.notify();
    }

    pub fn native_credential_edits(
        &self,
        cx: &gpui::App,
    ) -> Result<Vec<NativeCredentialEdit>, String> {
        if self.native_credential_task.is_some() {
            return Err(t!("auth.keychain_loading").to_string());
        }
        if self.native_credential_scope.as_ref() != Some(&self.credential_scope(cx)) {
            return Ok(Vec::new());
        }
        self.native_credentials
            .iter()
            .filter_map(|fields| {
                let mode = fields.original.mode;
                let value = if mode == AuthenticationMode::SystemAccount {
                    removent_proto::SystemCredentials {
                        username: fields.username.read(cx).value().trim().to_owned(),
                        password: fields.password.read(cx).value().to_string(),
                    }
                    .encode()
                } else {
                    fields.password.read(cx).value().to_string()
                };
                let answer = AuthenticationAnswer {
                    value,
                    remember: fields.remember,
                };
                if answer == fields.original.answer {
                    None
                } else if answer.remember && !mode.valid_input(&answer.value) {
                    Some(Err(t!("auth.saved_credentials_invalid").to_string()))
                } else {
                    Some(Ok(NativeCredentialEdit {
                        original: fields.original.clone(),
                        answer,
                    }))
                }
            })
            .collect()
    }

    pub(super) fn render_native_credentials(&self, cx: &mut Context<Self>) -> gpui::Div {
        let colors = cx.theme().colors;
        let mut body = div().flex().flex_col().gap_4();
        if self.native_credential_task.is_some() {
            return body.child(
                div()
                    .text_size(px(12.))
                    .text_color(colors.muted_foreground)
                    .child(t!("auth.keychain_loading").to_string()),
            );
        }
        if self.native_credential_scope.as_ref() != Some(&self.credential_scope(cx)) {
            return body;
        }
        for (index, fields) in self.native_credentials.iter().enumerate() {
            let system = fields.original.mode == AuthenticationMode::SystemAccount;
            let disabled = self.connecting || !fields.remember;
            body = body.child(
                div()
                    .debug_selector(move || format!("native-credential-{index}"))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_3()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(colors.border)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div().flex_1().min_w_0().text_size(px(13.)).child(
                                    t!(if system {
                                        "auth.system"
                                    } else {
                                        "auth.password"
                                    })
                                    .to_string(),
                                ),
                            )
                            .child(
                                Switch::new(("native-credential-remember", index))
                                    .small()
                                    .checked(fields.remember)
                                    .disabled(self.connecting)
                                    .on_click(cx.listener(move |this, checked, _, cx| {
                                        this.native_credentials[index].remember = *checked;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(colors.muted_foreground)
                            .child(t!("auth.saved_credentials_hint").to_string()),
                    )
                    .when(system, |el| {
                        el.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .child(t!("connection.username").to_string()),
                                )
                                .child(form_input(&fields.username).disabled(disabled)),
                        )
                    })
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
                            .child(
                                form_input(&fields.password)
                                    .mask_toggle()
                                    .disabled(disabled),
                            ),
                    ),
            );
        }
        body
    }
}
