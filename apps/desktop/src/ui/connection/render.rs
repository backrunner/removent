use super::*;

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
