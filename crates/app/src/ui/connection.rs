//! Two-level connection dialog: protocol, then session-specific connection details.

use gpui::{
    Context, Entity, EventEmitter, FocusHandle, Focusable, Render, Window, div, prelude::*, px,
};
use gpui_component::{
    ActiveTheme, Disableable, Sizable,
    button::{Button, ButtonVariants},
    input::{InputEvent, InputState},
    spinner::Spinner,
    switch::Switch,
};
use removent_client::connection::{
    AddressError, ConnectionAddress, ConnectionProtocol, ConnectionRequest,
};
use rust_i18n::t;

use super::widgets::{form_input, icon_16};

gpui::actions!(connection, [ConnectionTab, ConnectionTabPrev]);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionDialogEvent {
    Submit,
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
    host: Entity<InputState>,
    port: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    domain: Entity<InputState>,
    accept_invalid_certificate: bool,
    connecting: bool,
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
        let mut subscriptions = Vec::new();
        for input in [&host, &port, &username, &password, &domain] {
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
        let focus = cx.focus_handle();
        window.focus(&focus);
        Self {
            step: Step::Protocol,
            host,
            port,
            username,
            password,
            domain,
            accept_invalid_certificate: false,
            connecting: false,
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
        self.error = None;
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
        if self.connecting || self.step == Step::Protocol {
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
        self.connecting = connecting;
        self.error = error;
        cx.notify();
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
                &self.host,
                &self.port,
                &self.username,
                &self.password,
                &self.domain,
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
        let address =
            ConnectionAddress::parse(&self.host.read(cx).value(), &self.port.read(cx).value())
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
        Ok(ConnectionRequest {
            protocol,
            address,
            username,
            password: self.password.read(cx).value().to_string(),
            domain: self.domain.read(cx).value().trim().to_string(),
            accept_invalid_certificate: self.accept_invalid_certificate,
        })
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.obscured || self.connecting || self.step == Step::Protocol {
            return;
        }
        match self.request(cx) {
            Ok(_) => cx.emit(ConnectionDialogEvent::Submit),
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
        let mut body = div().flex().flex_col().gap_4();
        match self.step {
            Step::Protocol => {
                for (i, protocol) in ConnectionProtocol::ALL.into_iter().enumerate() {
                    body = body.child(
                        Button::new(("connection-protocol", i))
                            .outline()
                            .w_full()
                            .h(px(48.))
                            .icon(icon_16("monitor"))
                            .label(protocol.label())
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
                body = body.child(
                    div()
                        .flex()
                        .gap_3()
                        .child(field(t!("connection.address").to_string(), &self.host).flex_1())
                        .child(
                            field(t!("connection.port").to_string(), &self.port)
                                .w(px(88.))
                                .flex_shrink_0(),
                        ),
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
                body = body
                    .when_some(self.error.clone(), |el, error| {
                        el.child(
                            div()
                                .text_size(px(12.))
                                .text_color(colors.danger)
                                .child(error),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .when(busy, |el| {
                                el.child(Spinner::new().small()).child(
                                    div()
                                        .flex_1()
                                        .text_size(px(12.))
                                        .child(t!("connection.connecting").to_string()),
                                )
                            })
                            .child(
                                Button::new("cancel-connection")
                                    .ghost()
                                    .label(t!("action.cancel").to_string())
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(ConnectionDialogEvent::Close)
                                    })),
                            )
                            .child(
                                Button::new("submit-connection")
                                    .primary()
                                    .label(t!("action.connect").to_string())
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                            ),
                    );
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
            .overflow_y_scroll()
            .p_5()
            .flex()
            .flex_col()
            .gap_5()
            .child(
                div()
                    .flex()
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
                            .text_size(px(15.))
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
                            .tooltip(t!("action.close").to_string())
                            .on_click(
                                cx.listener(|_, _, _, cx| cx.emit(ConnectionDialogEvent::Close)),
                            ),
                    ),
            )
            .child(body)
    }
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
                .child(Button::new("background-control").label("Background"))
                .child(self.0.clone())
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
    fn keyboard_focus_stays_in_the_dialog_at_minimum_window_size(cx: &mut TestAppContext) {
        let (dialog, cx) = setup(cx);
        cx.simulate_resize(gpui::size(px(720.), px(480.)));
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
        assert_eq!(events.try_recv().unwrap(), ConnectionDialogEvent::Close);
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
