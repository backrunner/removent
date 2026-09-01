//! Viewer session window: immersive picture + floating toolbar + health badge (ui-design §4.2).

use crate::engine::{Engine, VideoFrame};
use crate::ui::motion;
use crate::ui::widgets::*;
use gpui::{
    AnimationExt, App, Bounds, Context, ElementId, FocusHandle, Keystroke, ModifiersChangedEvent,
    MouseButton, ObjectFit, Point, Render, RenderImage, Subscription, Task, TouchPhase, Window,
    WindowOptions, actions, div, img, prelude::*, px, size,
};
use gpui_component::{
    ActiveTheme, TITLE_BAR_HEIGHT, TitleBar,
    button::{Button, ButtonVariants},
};
use removent_proto::{KeyKind, KeyModifiers, MouseKind, ScrollPhase};
use rust_i18n::t;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

actions!(viewer, [ViewerEscape, ViewerToggleFullscreen]);

const TOOLBAR_HIDE_AFTER: Duration = Duration::from_millis(800);
/// Moving the mouse to the top window edge (y < 8px, window coordinates) reveals the toolbar.
const TOOLBAR_SHOW_EDGE: f32 = 8.;
/// After revealing, the hide timer keeps refreshing while the mouse stays in the top area
/// (including the toolbar itself). Window coordinate semantics: TitleBar 34px + toolbar top
/// 42px + ~28px height + 12px slack; the threshold must cover the entire toolbar button area,
/// otherwise hovering a button would hide the toolbar and mis-intercept the click.
const TOOLBAR_HOVER_AREA: f32 = 116.;

/// (modifier bit, left-variant macOS virtual key code); shared by
/// forward_modifiers (state diffing) and release_held_inputs (focus loss).
const MOD_KEYS: [(KeyModifiers, u16); 6] = [
    (KeyModifiers::SHIFT, 0x38),
    (KeyModifiers::CONTROL, 0x3B),
    (KeyModifiers::OPTION, 0x3A),
    (KeyModifiers::COMMAND, 0x37),
    (KeyModifiers::FUNCTION, 0x3F),
    (KeyModifiers::CAPS_LOCK, 0x39),
];

pub struct ViewerView {
    engine: Engine,
    current: Option<Arc<RenderImage>>,
    width: u32,
    height: u32,
    fps_counter: u32,
    fps_shown: u32,
    last_fps_tick: Instant,
    peer_name: String,
    toolbar_until: Option<Instant>,
    /// Bumped each time the toolbar transitions hidden → visible: folded into the animation
    /// element id so the entry transition replays on every appearance.
    toolbar_seq: u64,
    ended: bool,
    focus: FocusHandle,
    /// Currently held remote mouse buttons (bit0 left, bit1 right, bit2 middle),
    /// mirrored into every outbound mouse event.
    buttons: u8,
    /// Modifier state last forwarded to the host; ModifiersChanged events are diffed
    /// against it to derive per-modifier FlagsChanged key events.
    sent_modifiers: KeyModifiers,
    /// Virtual key codes forwarded as Down but not yet released as Up. macOS
    /// delivers key-up only to the key window, so these must be released
    /// explicitly when the window loses focus.
    pressed_keys: HashSet<u16>,
    /// Last forwarded remote cursor position (frame pixels); reused as the
    /// position for focus-loss button releases.
    last_mouse: Option<(f32, f32)>,
    /// Bumped each time the toolbar hide timer is (re-)armed; stale timers
    /// compare against it and no-op.
    toolbar_hide_seq: u64,
    /// Pending local-clipboard clear (drop = cancel); armed on window focus loss.
    clip_clear_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl ViewerView {
    pub fn new(
        engine: Engine,
        frames_rx: Receiver<VideoFrame>,
        peer_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);

        // std::mpsc → async bridge: only wakes the UI when a new frame arrives.
        // Previously this was a self-rescheduling on_next_frame poll per frame, which kept
        // spinning at refresh rate even when idle or disconnected.
        let (tx_async, mut rx_async) = futures::channel::mpsc::unbounded::<VideoFrame>();
        let bridge = std::thread::Builder::new()
            .name("viewer-frame-bridge".into())
            .spawn(move || {
                // After the peer disconnects, recv returns Err; the bridge thread exits and
                // the async-side stream ends accordingly.
                while let Ok(frame) = frames_rx.recv() {
                    if tx_async.unbounded_send(frame).is_err() {
                        break;
                    }
                }
            });
        // spawn_in + update_in: obtain the Window handle and refresh the window explicitly
        // after handling (entity notify does not bubble up to the window root Root
        // automatically; window.refresh() is required).
        let mut ended = false;
        match bridge {
            Ok(_) => {
                cx.spawn_in(
                    window,
                    async move |this: gpui::WeakEntity<ViewerView>, cx| {
                        use futures::StreamExt;
                        while let Some(first) = rx_async.next().await {
                            // Keep only the latest frame (drop the backlog); render latency takes priority.
                            let mut latest = first;
                            while let Ok(f) = rx_async.try_recv() {
                                latest = f;
                            }
                            let r = this.update_in(&mut *cx, |this, window, cx| {
                                this.handle_frame(latest);
                                if this.last_fps_tick.elapsed() >= Duration::from_secs(1) {
                                    this.fps_shown = this.fps_counter;
                                    this.fps_counter = 0;
                                    this.last_fps_tick = Instant::now();
                                }
                                cx.notify();
                                window.refresh();
                            });
                            if r.is_err() {
                                break;
                            }
                        }
                        // Stream ended: peer disconnected (bridge thread exited) or the view was destroyed.
                        let _ = this.update_in(&mut *cx, |this, window, cx| {
                            this.ended = true;
                            this.clip_clear_task = None;
                            this.pressed_keys.clear();
                            this.buttons = 0;
                            cx.notify();
                            window.refresh();
                        });
                    },
                )
                .detach();
            }
            // Thread spawn failure is unrecoverable for this session: show the
            // ended overlay instead of panicking the whole app.
            Err(e) => {
                tracing::error!(err=%e, "viewer frame bridge spawn failed");
                ended = true;
            }
        }

        // Focus-loss handling: release inputs still held remotely (macOS delivers
        // key-up only to the key window) and arm the local-clipboard clear timer
        // (settings.clipboard_clear_after_secs, 0 = disabled; cancelled on
        // reactivation or session end).
        let clear_secs = engine.settings().clipboard_clear_after_secs;
        let sub_activation = cx.observe_window_activation(window, move |this, window, cx| {
            if window.is_window_active() {
                // Focus regained: dropping the task cancels the pending clear.
                this.clip_clear_task = None;
                return;
            }
            this.release_held_inputs();
            if clear_secs == 0 || this.ended || this.clip_clear_task.is_some() {
                return;
            }
            let engine = this.engine.clone();
            this.clip_clear_task = Some(cx.spawn(async move |_this, cx| {
                cx.background_executor()
                    .timer(Duration::from_secs(clear_secs))
                    .await;
                // Clears the LOCAL clipboard; the remote side is not touched.
                engine.clear_local_clipboard();
            }));
        });

        Self {
            engine,
            current: None,
            width: 0,
            height: 0,
            fps_counter: 0,
            fps_shown: 0,
            last_fps_tick: Instant::now(),
            peer_name,
            toolbar_until: None,
            toolbar_seq: 0,
            ended,
            focus,
            buttons: 0,
            sent_modifiers: KeyModifiers::empty(),
            pressed_keys: HashSet::new(),
            last_mouse: None,
            toolbar_hide_seq: 0,
            clip_clear_task: None,
            _subscriptions: vec![sub_activation],
        }
    }

    /// Apply one frame: BGRA → RGBA conversion and RenderImage construction.
    fn handle_frame(&mut self, frame: VideoFrame) {
        if frame.width == 0 || frame.height == 0 {
            return;
        }
        let mut rgba = frame.data;
        for px4 in rgba.as_chunks_mut::<4>().0 {
            px4.swap(0, 2); // BGRA → RGBA
        }
        if let Some(buf) = image::RgbaImage::from_raw(frame.width, frame.height, rgba) {
            self.current = Some(Arc::new(RenderImage::new(vec![image::Frame::new(buf)])));
            self.width = frame.width;
            self.height = frame.height;
            self.fps_counter += 1;
        }
    }

    fn disconnect(&mut self, window: &mut Window) {
        self.clip_clear_task = None;
        self.pressed_keys.clear();
        self.buttons = 0;
        self.engine.disconnect_client();
        window.remove_window();
    }

    /// Input is forwarded only while a live session is streaming frames.
    fn input_active(&self) -> bool {
        !self.ended && self.width > 0 && self.height > 0
    }

    /// Map a window point to remote frame pixels, accounting for the title bar and
    /// the ObjectFit::Contain letterboxing. `clamp` selects out-of-bounds handling:
    /// false → None (letterbox/title-bar points must not be sent), true → clamped to
    /// the frame edge (button releases may land outside the picture and the host must
    /// still see them, or the button stays pressed remotely).
    ///
    /// Note: gpui exposes no way to read the CGEvent source user-data of an incoming
    /// event, so two Macs controlling each other cannot detect re-injected events
    /// here (loop protection would have to live at the CGEventTap level).
    fn frame_px(
        &self,
        pos: Point<gpui::Pixels>,
        window: &Window,
        clamp: bool,
    ) -> Option<(f32, f32)> {
        if !self.input_active() {
            return None;
        }
        let vp = window.viewport_size();
        let title = f32::from(TITLE_BAR_HEIGHT);
        let content_w = f32::from(vp.width);
        let content_h = f32::from(vp.height) - title;
        if content_w <= 0. || content_h <= 0. {
            return None;
        }
        let (fw, fh) = (self.width as f32, self.height as f32);
        // Contain scale (window points per frame pixel), then the letterbox offsets.
        let scale = (content_w / fw).min(content_h / fh);
        let ox = (content_w - fw * scale) / 2.;
        let oy = title + (content_h - fh * scale) / 2.;
        let x = (f32::from(pos.x) - ox) / scale;
        let y = (f32::from(pos.y) - oy) / scale;
        if clamp {
            // Wire coordinates are capture-frame pixels; the host remaps them
            // into display coordinates (capture dims → display dims).
            Some((x.clamp(0., fw - 1.), y.clamp(0., fh - 1.)))
        } else if x < 0. || y < 0. || x >= fw || y >= fh {
            None
        } else {
            Some((x, y))
        }
    }

    /// Forward a mouse event; `kind` drag variants are derived host-side from the
    /// `buttons` bitmask, so plain Moved/Down/Up is enough here.
    fn forward_mouse(&mut self, pos: Point<gpui::Pixels>, kind: MouseKind, window: &Window) {
        let clamp = matches!(
            kind,
            MouseKind::LeftUp | MouseKind::RightUp | MouseKind::MiddleUp
        );
        if let Some((x, y)) = self.frame_px(pos, window, clamp) {
            // 0 = main display (the host captures the main display only).
            self.last_mouse = Some((x, y));
            self.engine.send_input_mouse(0, x, y, self.buttons, kind);
        }
    }

    /// Forward a key down/up. The host prefers the `unicode` payload for text entry
    /// (client layout wins), so vk_code only needs to be right for command and
    /// navigation keys.
    fn forward_key(&mut self, keystroke: &Keystroke, kind: KeyKind) {
        if !self.input_active() {
            return;
        }
        let key = keystroke.key.as_str();
        // Bare modifier presses arrive as ModifiersChanged, handled separately.
        if matches!(key, "shift" | "control" | "alt" | "platform" | "function") {
            return;
        }
        let unicode = keystroke.key_char.as_deref().and_then(|s| {
            let mut chars = s.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Some(c),
                _ => None,
            }
        });
        let Some(vk_code) = mac_vk_of_key(key).or(unicode.map(|_| removent_input::UNICODE_ONLY_VK))
        else {
            return;
        };
        self.engine
            .send_input_key(vk_code, gpui_modifiers(&keystroke.modifiers), kind, unicode);
        // Track forwarded-but-unreleased keys so a focus loss can release them
        // (auto-repeat Down events re-insert the same vk, which is idempotent).
        match kind {
            KeyKind::Down => {
                self.pressed_keys.insert(vk_code);
            }
            KeyKind::Up => {
                self.pressed_keys.remove(&vk_code);
            }
            KeyKind::FlagsChanged => {}
        }
    }

    /// Release every key, modifier and mouse button still held remotely. Key-up
    /// is delivered only to the key window on macOS, so anything held while the
    /// window deactivates would stay stuck on the host: plain keys auto-repeat,
    /// and a stuck Command turns every local click into Cmd+click.
    fn release_held_inputs(&mut self) {
        let modifiers = self.sent_modifiers;
        for vk_code in self.pressed_keys.drain() {
            self.engine
                .send_input_key(vk_code, modifiers, KeyKind::Up, None);
        }
        // A modifier released while unfocused produces no ModifiersChanged here,
        // so release every bit still set (same FlagsChanged path as
        // forward_modifiers, with an empty modifier state).
        if !modifiers.is_empty() {
            for (bit, vk) in MOD_KEYS {
                if modifiers.contains(bit) {
                    self.engine.send_input_key(
                        vk,
                        KeyModifiers::empty(),
                        KeyKind::FlagsChanged,
                        None,
                    );
                }
            }
            self.sent_modifiers = KeyModifiers::empty();
        }
        if let Some((x, y)) = self.last_mouse {
            // (button bit, release kind); the bitmask shrinks as we go so the
            // host-side drag derivation unwinds cleanly.
            const HELD: [(u8, MouseKind); 3] = [
                (0x1, MouseKind::LeftUp),
                (0x2, MouseKind::RightUp),
                (0x4, MouseKind::MiddleUp),
            ];
            for (bit, kind) in HELD {
                if self.buttons & bit != 0 {
                    self.buttons &= !bit;
                    self.engine.send_input_mouse(0, x, y, self.buttons, kind);
                }
            }
        } else {
            self.buttons = 0;
        }
    }

    /// Arm the toolbar auto-hide timer. Rendering alone cannot hide the toolbar:
    /// when the remote screen is still, no frames or mouse events arrive to
    /// trigger a re-render, so the deadline is enforced by a background timer
    /// that wakes the view. `toolbar_hide_seq` invalidates superseded timers.
    fn arm_toolbar_hide_timer(&mut self, cx: &mut Context<Self>) {
        let Some(until) = self.toolbar_until else {
            return;
        };
        self.toolbar_hide_seq += 1;
        let seq = self.toolbar_hide_seq;
        let delay = until.saturating_duration_since(Instant::now());
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |this, cx| {
                if this.toolbar_hide_seq != seq {
                    return;
                }
                match this.toolbar_until {
                    // Refreshed while the timer was pending: re-arm for the new deadline.
                    Some(until) if Instant::now() < until => this.arm_toolbar_hide_timer(cx),
                    _ => {
                        this.toolbar_until = None;
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// Modifier keys produce no key down/up on macOS; diff the new modifier state
    /// against the last forwarded one and emit FlagsChanged for each changed modifier.
    fn forward_modifiers(&mut self, ev: &ModifiersChangedEvent) {
        if !self.input_active() {
            return;
        }
        let mut new = gpui_modifiers(&ev.modifiers);
        if ev.capslock.on {
            new |= KeyModifiers::CAPS_LOCK;
        }
        // (modifier bit, left-variant macOS virtual key code)
        for (bit, vk) in MOD_KEYS {
            if new.contains(bit) != self.sent_modifiers.contains(bit) {
                self.engine
                    .send_input_key(vk, new, KeyKind::FlagsChanged, None);
            }
        }
        self.sent_modifiers = new;
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        div()
            .absolute()
            .top(px(42.))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                overlay_chip()
                    .bg(colors.overlay)
                    .border_1()
                    .border_color(colors.border)
                    .shadow_md()
                    .child(
                        Button::new("toggle-fullscreen")
                            .icon(icon_16("maximize"))
                            .ghost()
                            .compact()
                            .tooltip(t!("viewer.fullscreen_tooltip").to_string())
                            .on_click(cx.listener(|_this, _, window, cx| {
                                window.toggle_fullscreen();
                                cx.notify();
                            })),
                    )
                    .child(div().w(px(1.)).h(px(16.)).mx_1().bg(colors.border))
                    .child(
                        Button::new("disconnect")
                            .icon(icon_16("power").text_color(colors.danger))
                            .ghost()
                            .compact()
                            .tooltip(t!("viewer.disconnect_tooltip").to_string())
                            .on_click(cx.listener(|this, _, window, _cx| {
                                this.disconnect(window);
                            })),
                    ),
            )
            // Entry transition: fade in while settling 8px down into place.
            .with_animation(
                ElementId::NamedInteger("viewer-toolbar".into(), self.toolbar_seq),
                motion::toolbar_enter(),
                |this, delta| this.top(px(34.) + delta * px(8.)).opacity(delta),
            )
    }

    fn render_badge(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let mono = cx.theme().mono_font_family.clone();
        let health = if self.width == 0 {
            Health::Unknown
        } else if self.fps_shown >= 24 {
            Health::Good
        } else if self.fps_shown >= 10 {
            Health::Fair
        } else {
            Health::Poor
        };
        let label = if self.width == 0 {
            t!("viewer.waiting").to_string()
        } else {
            format!("{}×{} · {} fps", self.width, self.height, self.fps_shown)
        };
        div().absolute().bottom(px(16.)).right(px(16.)).child(
            overlay_chip()
                .bg(colors.overlay)
                .border_1()
                .border_color(colors.border)
                .child(dot(health.color(cx)))
                .child(
                    div()
                        .text_size(px(11.))
                        .font_family(mono)
                        .text_color(colors.muted_foreground)
                        .child(label),
                ),
        )
    }

    fn render_ended_overlay(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        div()
            .absolute()
            .inset_0()
            .bg(crate::theme::scrim(cx.theme().is_dark()))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(320.))
                    .p_5()
                    .rounded(px(14.))
                    .bg(colors.popover)
                    .border_1()
                    .border_color(colors.border)
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_3()
                    .child(icon("alert-triangle").text_color(colors.warning))
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(t!("viewer.ended_title").to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(colors.muted_foreground)
                            .child(t!("viewer.ended_desc").to_string()),
                    )
                    .child(
                        Button::new("close-viewer")
                            .label(t!("viewer.close_window").to_string())
                            .primary()
                            .on_click(cx.listener(|_this, _, window, _cx| {
                                window.remove_window();
                            })),
                    ),
            )
            // Entry transition: the whole overlay fades in.
            .with_animation(
                "viewer-ended-fade",
                motion::overlay_fade(),
                |this, delta| this.opacity(delta),
            )
    }
}

impl Drop for ViewerView {
    fn drop(&mut self) {
        // Ensure the session task is terminated when the window is closed
        // (traffic lights / Esc / button).
        self.engine.disconnect_client();
    }
}

impl Render for ViewerView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                if toolbar_visible && y < TOOLBAR_HOVER_AREA {
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
                    this.buttons |= 0x2;
                    this.forward_mouse(ev.position, MouseKind::RightDown, window);
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(|this, ev: &gpui::MouseDownEvent, window, _cx| {
                    this.buttons |= 0x4;
                    this.forward_mouse(ev.position, MouseKind::MiddleDown, window);
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, ev: &gpui::MouseUpEvent, window, _cx| {
                    this.buttons &= !0x1;
                    this.forward_mouse(ev.position, MouseKind::LeftUp, window);
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, ev: &gpui::MouseUpEvent, window, _cx| {
                    this.buttons &= !0x2;
                    this.forward_mouse(ev.position, MouseKind::RightUp, window);
                }),
            )
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(|this, ev: &gpui::MouseUpEvent, window, _cx| {
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
                    .child(self.render_badge(cx))
                    .when(self.ended, |el| el.child(self.render_ended_overlay(cx))),
            )
    }
}

/// gpui modifiers → wire bitmask.
fn gpui_modifiers(m: &gpui::Modifiers) -> KeyModifiers {
    let mut out = KeyModifiers::empty();
    if m.shift {
        out |= KeyModifiers::SHIFT;
    }
    if m.control {
        out |= KeyModifiers::CONTROL;
    }
    if m.alt {
        out |= KeyModifiers::OPTION;
    }
    if m.platform {
        out |= KeyModifiers::COMMAND;
    }
    if m.function {
        out |= KeyModifiers::FUNCTION;
    }
    out
}

/// gpui key name → macOS virtual key code (ANSI layout). Text entry rides on the
/// `unicode` payload host-side, so this table only needs to cover command and
/// navigation keys accurately; unmapped printable keys return None and are sent
/// with the UNICODE_ONLY_VK sentinel + unicode by the caller.
fn mac_vk_of_key(key: &str) -> Option<u16> {
    Some(match key {
        "a" => 0x00,
        "s" => 0x01,
        "d" => 0x02,
        "f" => 0x03,
        "h" => 0x04,
        "g" => 0x05,
        "z" => 0x06,
        "x" => 0x07,
        "c" => 0x08,
        "v" => 0x09,
        "b" => 0x0B,
        "q" => 0x0C,
        "w" => 0x0D,
        "e" => 0x0E,
        "r" => 0x0F,
        "y" => 0x10,
        "t" => 0x11,
        "1" => 0x12,
        "2" => 0x13,
        "3" => 0x14,
        "4" => 0x15,
        "6" => 0x16,
        "5" => 0x17,
        "=" => 0x18,
        "9" => 0x19,
        "7" => 0x1A,
        "-" => 0x1B,
        "8" => 0x1C,
        "0" => 0x1D,
        "]" => 0x1E,
        "o" => 0x1F,
        "u" => 0x20,
        "[" => 0x21,
        "i" => 0x22,
        "p" => 0x23,
        "enter" => 0x24,
        "l" => 0x25,
        "j" => 0x26,
        "'" => 0x27,
        "k" => 0x28,
        ";" => 0x29,
        "\\" => 0x2A,
        "," => 0x2B,
        "/" => 0x2C,
        "n" => 0x2D,
        "m" => 0x2E,
        "." => 0x2F,
        "tab" => 0x30,
        "space" => 0x31,
        "`" => 0x32,
        "backspace" => 0x33,
        // Dead entry in practice: Esc is intercepted by the local ViewerEscape
        // key binding and never forwarded to the host. Kept for completeness.
        "escape" => 0x35,
        "f17" => 0x40,
        "f18" => 0x4F,
        "f19" => 0x50,
        "f20" => 0x5A,
        "f5" => 0x60,
        "f6" => 0x61,
        "f7" => 0x62,
        "f3" => 0x63,
        "f8" => 0x64,
        "f9" => 0x65,
        "f11" => 0x67,
        "f13" => 0x69,
        "f16" => 0x6A,
        "f14" => 0x6B,
        "f10" => 0x6D,
        "f12" => 0x6F,
        "f15" => 0x71,
        "insert" => 0x72,
        "home" => 0x73,
        "pageup" => 0x74,
        "delete" => 0x75,
        "f4" => 0x76,
        "end" => 0x77,
        "f2" => 0x78,
        "pagedown" => 0x79,
        "f1" => 0x7A,
        "left" => 0x7B,
        "right" => 0x7C,
        "down" => 0x7D,
        "up" => 0x7E,
        _ => return None,
    })
}

/// Open the Viewer window and take over the frame channel.
pub fn open_viewer_window(
    engine: Engine,
    frames_rx: Receiver<VideoFrame>,
    peer_name: String,
    cx: &mut App,
) -> Result<(), String> {
    let bounds = Bounds::centered(None, size(px(1280.), px(720.)), cx);
    let handle = cx.open_window(
        WindowOptions {
            window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
            titlebar: Some(TitleBar::title_bar_options()),
            window_min_size: Some(size(px(480.), px(320.))),
            ..Default::default()
        },
        move |window, cx| {
            let view = cx.new(|cx| ViewerView::new(engine, frames_rx, peer_name, window, cx));
            cx.new(|cx| gpui_component::Root::new(gpui::AnyView::from(view), window, cx))
        },
    );
    handle.map(|_| ()).map_err(|e| e.to_string())
}
