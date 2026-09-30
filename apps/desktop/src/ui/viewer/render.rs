use super::*;

impl Render for ViewerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let toolbar_visible = !self.ended
            && self
                .toolbar_until
                .is_some_and(|until| Instant::now() < until);

        let colors = cx.theme().colors;
        // Text/icons on the fixed dark surface (0x05070A background) do not use theme
        // tokens — muted_foreground lacks contrast on a dark base in the light theme;
        // a fixed light color is used uniformly.
        let on_dark = gpui::white().opacity(0.65);
        div()
            .key_context("Viewer")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &ViewerEscape, window, _cx| {
                if window.is_fullscreen() {
                    window.toggle_fullscreen();
                } else {
                    this.disconnect(window);
                }
            }))
            .on_action(
                cx.listener(|_this, _: &ViewerToggleFullscreen, window, cx| {
                    window.toggle_fullscreen();
                    cx.notify();
                }),
            )
            .on_action(cx.listener(|this, _: &ViewerToggleInfo, window, cx| {
                this.toggle_info(window, cx);
            }))
            .on_mouse_move(cx.listener(|this, ev: &gpui::MouseMoveEvent, window, cx| {
                let y = f32::from(ev.position.y);
                let now = Instant::now();
                let toolbar_visible = this.toolbar_until.is_some_and(|until| now < until);
                let show = y < TOOLBAR_SHOW_EDGE
                    || (y < TOOLBAR_HOVER_AREA && this.toolbar_until.is_some());
                if show {
                    if !toolbar_visible {
                        this.toolbar_seq += 1;
                    }
                    this.toolbar_until = Some(now + TOOLBAR_HIDE_AFTER);
                    // Arm the hide timer on the hidden → visible transition only;
                    // later refreshes are picked up when the timer re-arms itself.
                    if !toolbar_visible {
                        this.arm_toolbar_hide_timer(cx);
                    }
                    cx.notify();
                }
                // Pointer over the visible toolbar: swallow the move so the remote
                // cursor does not jump to the top of the picture (down/up keep the
                // existing toolbar click interception and release clamping).
                if toolbar_visible && y < TOOLBAR_HOVER_AREA && this.buttons == 0 {
                    return;
                }
                // Drag kinds are derived host-side from the buttons bitmask.
                this.forward_mouse(ev.position, MouseKind::Moved, window);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &gpui::MouseDownEvent, window, cx| {
                    // Clicking elsewhere on the picture collapses the toolbar immediately
                    // while it is visible.
                    let on_picture = f32::from(ev.position.y) >= TOOLBAR_HOVER_AREA
                        || this.toolbar_until.is_none();
                    if !on_picture {
                        return;
                    }
                    if this.frame_px(ev.position, window, false).is_none() {
                        return;
                    }
                    if this.toolbar_until.is_some() {
                        this.toolbar_until = None;
                        cx.notify();
                    }
                    this.buttons |= 0x1;
                    this.forward_mouse(ev.position, MouseKind::LeftDown, window);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, ev: &gpui::MouseDownEvent, window, _cx| {
                    if this.frame_px(ev.position, window, false).is_none() {
                        return;
                    }
                    this.buttons |= 0x2;
                    this.forward_mouse(ev.position, MouseKind::RightDown, window);
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(|this, ev: &gpui::MouseDownEvent, window, _cx| {
                    if this.frame_px(ev.position, window, false).is_none() {
                        return;
                    }
                    this.buttons |= 0x4;
                    this.forward_mouse(ev.position, MouseKind::MiddleDown, window);
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, ev: &gpui::MouseUpEvent, window, _cx| {
                    if this.buttons & 0x1 == 0 {
                        return;
                    }
                    this.buttons &= !0x1;
                    this.forward_mouse(ev.position, MouseKind::LeftUp, window);
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, ev: &gpui::MouseUpEvent, window, _cx| {
                    if this.buttons & 0x2 == 0 {
                        return;
                    }
                    this.buttons &= !0x2;
                    this.forward_mouse(ev.position, MouseKind::RightUp, window);
                }),
            )
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(|this, ev: &gpui::MouseUpEvent, window, _cx| {
                    if this.buttons & 0x4 == 0 {
                        return;
                    }
                    this.buttons &= !0x4;
                    this.forward_mouse(ev.position, MouseKind::MiddleUp, window);
                }),
            )
            .on_scroll_wheel(
                cx.listener(|this, ev: &gpui::ScrollWheelEvent, window, _cx| {
                    // Only scroll when the pointer is over the remote picture; the wire
                    // event carries no position (the host scrolls at its own cursor,
                    // which tracks our forwarded mouse moves).
                    if this.frame_px(ev.position, window, false).is_none() {
                        return;
                    }
                    // The protocol carries millimetres; convert logical px at 96 dpi
                    // (line deltas via a 16px line height approximation).
                    const MM_PER_PX: f32 = 25.4 / 96.;
                    let delta = ev.delta.pixel_delta(px(16.));
                    // Negate: gpui's delta is positive when scrolling up, while the host
                    // side negates again for CGEvent — keeping the remote direction
                    // identical to the local gesture.
                    let dx_mm = -f32::from(delta.x) * MM_PER_PX;
                    let dy_mm = -f32::from(delta.y) * MM_PER_PX;
                    let phase = match ev.touch_phase {
                        TouchPhase::Started => ScrollPhase::Began,
                        TouchPhase::Moved => ScrollPhase::Changed,
                        TouchPhase::Ended => ScrollPhase::Ended,
                    };
                    this.engine.send_input_scroll(0, dx_mm, dy_mm, phase);
                }),
            )
            .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, _w, _cx| {
                this.forward_key(&ev.keystroke, KeyKind::Down);
            }))
            .on_key_up(cx.listener(|this, ev: &gpui::KeyUpEvent, _w, _cx| {
                this.forward_key(&ev.keystroke, KeyKind::Up);
            }))
            .on_modifiers_changed(cx.listener(|this, ev: &ModifiersChangedEvent, _w, _cx| {
                this.forward_modifiers(ev);
            }))
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(gpui::rgb(0x05070A))
            .text_color(colors.foreground)
            .child(
                TitleBar::new()
                    .bg(gpui::rgb(0x05070A))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(px(12.))
                            .text_color(on_dark)
                            .child(self.peer_name.clone()),
                    )
                    .child(div()),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .overflow_hidden()
                    .child(match self.current.clone() {
                        Some(image) => div()
                            .size_full()
                            .child(img(image).size_full().object_fit(ObjectFit::Contain))
                            .into_any_element(),
                        None => div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .gap_3()
                                    .child(icon("loader-circle").text_color(on_dark))
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(on_dark)
                                            .child(t!("viewer.waiting_stream").to_string()),
                                    ),
                            )
                            .into_any_element(),
                    })
                    .when(toolbar_visible, |el| el.child(self.render_toolbar(cx)))
                    .when(self.info_visible && !self.ended, |el| {
                        el.child(self.render_info(window, cx))
                    })
                    .when(self.ended, |el| el.child(self.render_ended_overlay(cx))),
            )
    }
}
