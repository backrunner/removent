//! Viewer session window: immersive picture, floating controls, optional diagnostics.

use crate::engine::{ClientDiagnostics, Engine, VideoFrame};
use crate::ui::motion;
use crate::ui::widgets::*;
use gpui::{
    AnimationExt, App, Bounds, Context, ElementId, FocusHandle, Keystroke, ModifiersChangedEvent,
    MouseButton, ObjectFit, Point, Render, RenderImage, Subscription, Task, TouchPhase, Window,
    WindowOptions, actions, div, img, prelude::*, px, size,
};
use gpui_component::{ActiveTheme, TITLE_BAR_HEIGHT, TitleBar};
use removent_core::latest::Receiver;
use removent_proto::{KeyKind, KeyModifiers, MouseKind, ScrollPhase};
use rust_i18n::t;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

actions!(
    viewer,
    [ViewerEscape, ViewerToggleFullscreen, ViewerToggleInfo]
);

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
    session_generation: usize,
    current: Option<Arc<RenderImage>>,
    width: u32,
    height: u32,
    fps_counter: u32,
    fps_shown: f64,
    last_fps_tick: Instant,
    info_visible: bool,
    info_scroll: gpui::ScrollHandle,
    started: Instant,
    last_frame: Option<Instant>,
    diagnostics: ClientDiagnostics,
    received_bytes: u64,
    received_frames: u64,
    receive_rate: f64,
    receive_fps: f64,
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
        mut frames_rx: Receiver<VideoFrame>,
        peer_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);

        // The single-slot channel wakes GPUI directly. No polling thread or
        // unbounded intermediary is needed while the window is busy.
        cx.spawn_in(
            window,
            async move |this: gpui::WeakEntity<ViewerView>, cx| {
                while let Some(frame) = frames_rx.recv().await {
                    if this
                        .update_in(&mut *cx, |this, window, cx| {
                            this.handle_frame(frame, window);
                            cx.notify();
                        })
                        .is_err()
                    {
                        break;
                    }
                    // Always return time to the UI event loop. The latest-frame
                    // channel replaces stale pictures while key/mouse events run.
                    cx.background_executor()
                        .timer(Duration::from_millis(16))
                        .await;
                }
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

        // Sampling is independent of frame arrivals: idle desktops show 0 fps,
        // and stalled updates still expose receive rate and queued input age.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if !this
                    .update(cx, |this, cx| {
                        if this.ended {
                            return false;
                        }
                        this.sample_diagnostics();
                        if this.info_visible {
                            cx.notify();
                        }
                        true
                    })
                    .unwrap_or(false)
                {
                    break;
                }
            }
        })
        .detach();

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
            session_generation: engine.client_generation(),
            engine,
            current: None,
            width: 0,
            height: 0,
            fps_counter: 0,
            fps_shown: 0.,
            last_fps_tick: Instant::now(),
            info_visible: false,
            info_scroll: gpui::ScrollHandle::new(),
            started: Instant::now(),
            last_frame: None,
            diagnostics: ClientDiagnostics::default(),
            received_bytes: 0,
            received_frames: 0,
            receive_rate: 0.,
            receive_fps: 0.,
            peer_name,
            toolbar_until: None,
            toolbar_seq: 0,
            ended: false,
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

    /// GPUI RenderImage consumes BGRA bytes, despite using an RgbaImage container.
    fn handle_frame(&mut self, frame: VideoFrame, window: &mut Window) {
        if frame.width == 0 || frame.height == 0 {
            return;
        }
        if let Some(buf) = image::RgbaImage::from_raw(frame.width, frame.height, frame.data) {
            let next = Arc::new(RenderImage::new(vec![image::Frame::new(buf)]));
            if let Some(previous) = self.current.replace(next) {
                // RenderImage IDs are unique. GPUI's sprite atlas does not
                // evict them when the Arc drops, so release each retired frame.
                let _ = window.drop_image(previous);
            }
            if self.width > 0
                && self.height > 0
                && let Some((x, y)) = &mut self.last_mouse
            {
                *x *= frame.width as f32 / self.width as f32;
                *y *= frame.height as f32 / self.height as f32;
            }
            self.engine
                .set_frame_geometry(self.session_generation, frame.width, frame.height);
            self.width = frame.width;
            self.height = frame.height;
            self.fps_counter += 1;
            self.last_frame = Some(Instant::now());
        }
    }

    fn sample_diagnostics(&mut self) {
        let seconds = self.last_fps_tick.elapsed().as_secs_f64().max(0.001);
        self.fps_shown = self.fps_counter as f64 / seconds;
        self.fps_counter = 0;
        self.last_fps_tick = Instant::now();
        self.diagnostics = self.engine.client_diagnostics(self.session_generation);
        if let Some(stats) = self.diagnostics.vnc {
            self.receive_rate =
                stats.received_bytes.saturating_sub(self.received_bytes) as f64 / seconds;
            self.receive_fps =
                stats.received_frames.saturating_sub(self.received_frames) as f64 / seconds;
            self.received_bytes = stats.received_bytes;
            self.received_frames = stats.received_frames;
        }
    }

    fn toggle_info(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.info_visible = !self.info_visible;
        self.diagnostics = self.engine.client_diagnostics(self.session_generation);
        window.focus(&self.focus);
        cx.notify();
    }

    fn disconnect(&mut self, window: &mut Window) {
        self.clip_clear_task = None;
        self.pressed_keys.clear();
        self.buttons = 0;
        self.engine.disconnect_client_if(self.session_generation);
        window.remove_window();
    }

    /// Input is forwarded only while a live session is streaming frames.
    fn input_active(&self) -> bool {
        !self.ended
            && self.engine.client_generation() == self.session_generation
            && self.width > 0
            && self.height > 0
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
        if !self.input_active() || (kind != KeyKind::Up && is_viewer_shortcut(keystroke)) {
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
        // A local shortcut can be released after its modifiers. Only release
        // physical keys whose down event was actually forwarded.
        if kind == KeyKind::Up && !self.pressed_keys.contains(&vk_code) {
            return;
        }
        let modifiers = key_modifiers(&keystroke.modifiers, self.sent_modifiers);
        self.engine
            .send_input_key(vk_code, modifiers, kind, unicode);
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
        if self.engine.client_generation() != self.session_generation {
            return;
        }
        let modifiers = self.sent_modifiers;
        for vk_code in self.pressed_keys.drain() {
            self.engine
                .send_input_key(vk_code, modifiers, KeyKind::Up, None);
        }
        // A modifier released while unfocused produces no ModifiersChanged here,
        // so release every bit still set (same FlagsChanged path as
        // forward_modifiers, clearing the held modifier flags).
        if !modifiers.is_empty() {
            // Caps Lock is a toggle, not a held key. Keep it across focus loss
            // so subsequent keystrokes retain the last observed lock state.
            let released = modifiers & KeyModifiers::CAPS_LOCK;
            for (bit, vk) in MOD_KEYS {
                if bit != KeyModifiers::CAPS_LOCK && modifiers.contains(bit) {
                    self.engine
                        .send_input_key(vk, released, KeyKind::FlagsChanged, None);
                }
            }
            self.sent_modifiers = released;
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
            .when(self.info_visible, |el| el.justify_end().pr_4())
            .child(
                overlay_chip(cx)
                    .debug_selector(|| "viewer-toolbar-chip".into())
                    .child(
                        Button::new("toggle-info")
                            .icon(icon_16("gauge"))
                            .segment(self.info_visible)
                            .compact()
                            .tooltip(t!("viewer.info_tooltip").to_string())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_info(window, cx);
                            })),
                    )
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
                            .icon(icon_16("power"))
                            .destructive()
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

    fn render_info(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mono = cx.theme().mono_font_family.clone();
        let muted = gpui::white().opacity(0.65);
        let mut rows = vec![
            (t!("viewer.info_peer").to_string(), self.peer_name.clone()),
            (
                t!("viewer.info_protocol").to_string(),
                self.diagnostics.codec.clone(),
            ),
            (
                t!("viewer.info_duration").to_string(),
                format_duration(self.started.elapsed()),
            ),
            (
                t!("viewer.info_resolution").to_string(),
                if self.width > 0 {
                    format!("{} × {}", self.width, self.height)
                } else {
                    "—".into()
                },
            ),
            (
                t!("viewer.info_display_fps").to_string(),
                format!("{:.1} fps", self.fps_shown),
            ),
            (
                t!("viewer.info_frame_age").to_string(),
                self.last_frame
                    .map(|last| format!("{:.1} s", last.elapsed().as_secs_f64()))
                    .unwrap_or_else(|| "—".into()),
            ),
        ];
        if let Some(stats) = self.diagnostics.vnc {
            rows.extend([
                (
                    t!("viewer.info_receive_fps").to_string(),
                    format!("{:.1} fps", self.receive_fps),
                ),
                (
                    t!("viewer.info_receive_rate").to_string(),
                    format!("{:.2} MB/s", self.receive_rate / 1_000_000.),
                ),
                (
                    t!("viewer.info_received").to_string(),
                    format!("{:.1} MB", stats.received_bytes as f64 / 1_000_000.),
                ),
                (
                    t!("viewer.info_decode").to_string(),
                    if stats.received_frames > 0 {
                        format!("{:.1} ms", stats.decode_us as f64 / 1000.)
                    } else {
                        "—".into()
                    },
                ),
                (
                    t!("viewer.info_update").to_string(),
                    if stats.received_frames > 0 {
                        format!("{:.1} ms", stats.update_us as f64 / 1000.)
                    } else {
                        "—".into()
                    },
                ),
            ]);
        }
        if let Some(input) = self.diagnostics.input {
            rows.extend([
                (
                    t!("viewer.info_input_pending").to_string(),
                    input.pending.to_string(),
                ),
                (
                    t!("viewer.info_input_age").to_string(),
                    format_ms(input.oldest_pending),
                ),
                (
                    t!("viewer.info_input_delay").to_string(),
                    format_ms(input.dispatch_delay),
                ),
                (
                    t!("viewer.info_input_sent").to_string(),
                    input.sent.to_string(),
                ),
                (
                    t!("viewer.info_coalesced").to_string(),
                    input.coalesced.to_string(),
                ),
            ]);
        }
        let max_height =
            (f32::from(window.viewport_size().height - TITLE_BAR_HEIGHT) - 32.).max(100.);
        div()
            .id("viewer-info")
            .debug_selector(|| "viewer-info".into())
            .absolute()
            .top(px(16.))
            .left(px(16.))
            .w(px(280.))
            .max_h(px(max_height))
            .flex()
            .flex_col()
            .rounded(px(12.))
            .bg(gpui::rgba(0x101418D1))
            .border_1()
            .border_color(gpui::white().opacity(0.14))
            .text_color(gpui::white().opacity(0.92))
            // Panel interaction is local. Mouse-up still bubbles so a remote
            // drag ending on the panel releases its held button.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
            .on_mouse_move(cx.listener(|this, _, _, cx| {
                if this.buttons == 0 {
                    cx.stop_propagation();
                }
            }))
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .flex_shrink_0()
                    .child(icon_16("gauge").text_color(muted))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.))
                            .child(t!("viewer.info_title").to_string()),
                    )
                    .child(
                        div()
                            .id("close-viewer-info")
                            .debug_selector(|| "close-viewer-info".into())
                            .cursor_pointer()
                            .p_1()
                            .rounded(px(4.))
                            .hover(|el| el.bg(gpui::white().opacity(0.12)))
                            .child(icon_16("x").text_color(muted))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_info(window, cx);
                                cx.stop_propagation();
                            })),
                    ),
            )
            .child(
                div()
                    .id("viewer-info-rows")
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.info_scroll)
                    .px_3()
                    .pb_3()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(muted)
                            .mb_2()
                            .child(t!("viewer.info_idle_hint").to_string()),
                    )
                    .children(rows.into_iter().map(|(label, value)| {
                        div()
                            .flex()
                            .items_start()
                            .gap_3()
                            .py_1()
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(11.))
                                    .text_color(muted)
                                    .child(label),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_right()
                                    .text_size(px(11.))
                                    .font_family(mono.clone())
                                    .child(value),
                            )
                    }))
                    .when(self.diagnostics.input.is_some(), |el| {
                        el.child(
                            div()
                                .text_size(px(11.))
                                .text_color(muted)
                                .mt_2()
                                .child(t!("viewer.info_input_hint").to_string()),
                        )
                    }),
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
                    .rounded(px(18.))
                    .bg(colors.popover)
                    .border_1()
                    .border_color(colors.border)
                    .shadow(crate::theme::popup_shadow(cx.theme().is_dark()))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_3()
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
        self.engine.disconnect_client_if(self.session_generation);
    }
}

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

/// Reserve only explicit local shortcuts; bare Escape and Cmd-F belong to
/// the remote application. Suppress both key-down and key-up forwarding.
fn is_viewer_shortcut(key: &Keystroke) -> bool {
    key.modifiers.control
        && key.modifiers.platform
        && matches!(key.key.as_str(), "escape" | "f" | "i")
}

fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

fn format_ms(duration: Option<Duration>) -> String {
    duration
        .map(|value| format!("{:.1} ms", value.as_secs_f64() * 1000.))
        .unwrap_or_else(|| "—".into())
}

/// gpui modifiers → wire bitmask.
fn key_modifiers(modifiers: &gpui::Modifiers, sent: KeyModifiers) -> KeyModifiers {
    // GPUI Keystroke modifiers omit Caps Lock; its state arrives separately.
    gpui_modifiers(modifiers) | (sent & KeyModifiers::CAPS_LOCK)
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AnyView, Entity, KeyBinding, TestAppContext, VisualTestContext, point};
    use removent_proto::ControlMsg;

    struct Fixture {
        viewer: Entity<ViewerView>,
        commands: tokio::sync::mpsc::Receiver<ControlMsg>,
        _frames: removent_core::latest::Sender<VideoFrame>,
        _directory: tempfile::TempDir,
    }

    fn setup(cx: &mut TestAppContext) -> (Fixture, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.bind_keys([KeyBinding::new(
                "ctrl-cmd-i",
                ViewerToggleInfo,
                Some("Viewer"),
            )]);
        });
        let directory = tempfile::tempdir().unwrap();
        let (commands, mut commands_rx) = tokio::sync::mpsc::channel(64);
        let engine = Engine::for_viewer_test(
            removent_core::DataPaths {
                root: directory.path().into(),
            },
            commands,
        );
        let (frames, frames_rx) = removent_core::latest::channel();
        let viewer_slot = std::rc::Rc::new(std::cell::RefCell::new(None));
        let slot = viewer_slot.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let viewer =
                cx.new(|cx| ViewerView::new(engine, frames_rx, "Remote Mac".into(), window, cx));
            viewer.update(cx, |view, _| {
                view.handle_frame(
                    VideoFrame {
                        width: 640,
                        height: 360,
                        data: vec![0; 640 * 360 * 4],
                        pts_us: 0,
                    },
                    window,
                );
            });
            *slot.borrow_mut() = Some(viewer.clone());
            gpui_component::Root::new(AnyView::from(viewer), window, cx)
        });
        let viewer = viewer_slot.borrow_mut().take().unwrap();
        while commands_rx.try_recv().is_ok() {}
        (
            Fixture {
                viewer,
                commands: commands_rx,
                _frames: frames,
                _directory: directory,
            },
            cx,
        )
    }

    #[gpui::test]
    fn info_toggle_and_panel_clicks_stay_local(cx: &mut TestAppContext) {
        let (mut fixture, cx) = setup(cx);
        cx.run_until_parked();
        assert!(cx.debug_bounds("viewer-info").is_none());
        cx.simulate_keystrokes("ctrl-cmd-i");
        cx.run_until_parked();
        let panel = cx.debug_bounds("viewer-info").unwrap();
        assert_eq!(panel.left(), px(16.));
        assert_eq!(panel.top(), TITLE_BAR_HEIGHT + px(16.));
        for button in [MouseButton::Left, MouseButton::Right, MouseButton::Middle] {
            cx.simulate_mouse_down(panel.center(), button, Default::default());
            cx.simulate_mouse_up(panel.center(), button, Default::default());
        }
        assert!(
            fixture.commands.try_recv().is_err(),
            "panel interactions must not move or click remotely"
        );
        let close = cx.debug_bounds("close-viewer-info").unwrap();
        cx.simulate_mouse_move(close.center(), None, Default::default());
        cx.simulate_click(close.center(), Default::default());
        cx.run_until_parked();
        cx.update(|_, cx| {
            fixture.viewer.update(cx, |view, _| {
                assert!(!view.info_visible);
                view.forward_key(&Keystroke::parse("i").unwrap(), KeyKind::Up);
            })
        });
        assert!(
            fixture.commands.try_recv().is_err(),
            "releasing a local shortcut must not send a stray key-up"
        );
        cx.update(|_, cx| {
            fixture.viewer.update(cx, |view, _| {
                view.forward_key(&Keystroke::parse("i").unwrap(), KeyKind::Down);
                view.forward_key(&Keystroke::parse("ctrl-cmd-i").unwrap(), KeyKind::Up);
            })
        });
        assert!(matches!(
            fixture.commands.try_recv().unwrap(),
            ControlMsg::KeyEvent {
                kind: KeyKind::Down,
                ..
            }
        ));
        assert!(
            matches!(
                fixture.commands.try_recv().unwrap(),
                ControlMsg::KeyEvent {
                    kind: KeyKind::Up,
                    ..
                }
            ),
            "a remotely held key must release even if local shortcut modifiers were added"
        );
    }

    #[gpui::test]
    fn releasing_a_remote_drag_over_info_does_not_leave_a_held_button(cx: &mut TestAppContext) {
        let (mut fixture, cx) = setup(cx);
        cx.simulate_resize(size(px(1000.), px(700.)));
        cx.simulate_keystrokes("ctrl-cmd-i");
        cx.run_until_parked();
        let panel = cx.debug_bounds("viewer-info").unwrap();
        cx.simulate_mouse_down(
            point(px(800.), px(350.)),
            MouseButton::Left,
            Default::default(),
        );
        cx.simulate_mouse_move(panel.center(), MouseButton::Left, Default::default());
        cx.simulate_mouse_up(panel.center(), MouseButton::Left, Default::default());
        let events: Vec<_> = std::iter::from_fn(|| fixture.commands.try_recv().ok()).collect();
        assert!(matches!(
            events.first(),
            Some(ControlMsg::MouseEvent {
                kind: MouseKind::LeftDown,
                buttons: 1,
                ..
            })
        ));
        assert!(matches!(
            events.last(),
            Some(ControlMsg::MouseEvent {
                kind: MouseKind::LeftUp,
                buttons: 0,
                ..
            })
        ));
        cx.update(|_, cx| {
            fixture
                .viewer
                .update(cx, |view, _| assert_eq!(view.buttons, 0))
        });
    }

    #[gpui::test]
    fn full_info_fits_minimum_window_and_idle_samples_reset_fps(cx: &mut TestAppContext) {
        let (mut fixture, cx) = setup(cx);
        cx.simulate_resize(size(px(480.), px(320.)));
        cx.simulate_keystrokes("ctrl-cmd-i");
        cx.update(|_, cx| {
            fixture.viewer.update(cx, |view, cx| {
                view.toolbar_until = Some(Instant::now() + TOOLBAR_HIDE_AFTER);
                view.diagnostics.input = Some(Default::default());
                view.diagnostics.vnc = Some(Default::default());
                cx.notify();
            })
        });
        cx.run_until_parked();
        let panel = cx.debug_bounds("viewer-info").unwrap();
        let close = cx.debug_bounds("close-viewer-info").unwrap();
        assert!(panel.bottom() <= px(320. - 16.), "{panel:?}");
        assert!(panel.right() <= px(480.));
        assert!(close.bottom() < panel.bottom());
        let toolbar = cx.debug_bounds("viewer-toolbar-chip").unwrap();
        assert!(
            toolbar.left() > panel.right(),
            "{toolbar:?} overlaps {panel:?}"
        );
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: panel.center(),
            delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-100.))),
            touch_phase: TouchPhase::Moved,
            modifiers: Default::default(),
        });
        assert!(
            fixture.commands.try_recv().is_err(),
            "scrolling performance info must stay local"
        );
        cx.update(|_, cx| {
            fixture.viewer.update(cx, |view, _| {
                assert!(
                    view.info_scroll.offset().y < px(0.),
                    "all diagnostics must be reachable by scrolling"
                );
                view.fps_counter = 30;
                view.last_fps_tick = Instant::now() - Duration::from_secs(2);
                view.sample_diagnostics();
                assert!((14.9..=15.1).contains(&view.fps_shown));
                view.last_fps_tick = Instant::now() - Duration::from_secs(1);
                view.sample_diagnostics();
                assert_eq!(view.fps_shown, 0.);
            })
        });
    }

    #[test]
    fn keystrokes_preserve_caps_lock_from_modifier_notifications() {
        let modifiers = gpui::Modifiers {
            shift: true,
            ..Default::default()
        };
        assert_eq!(
            key_modifiers(&modifiers, KeyModifiers::CAPS_LOCK),
            KeyModifiers::SHIFT | KeyModifiers::CAPS_LOCK
        );
        assert_eq!(
            key_modifiers(&modifiers, KeyModifiers::empty()),
            KeyModifiers::SHIFT
        );
    }

    #[test]
    fn escape_and_find_belong_to_remote_except_explicit_local_chords() {
        for (key, local) in [
            ("escape", false),
            ("cmd-f", false),
            ("ctrl-cmd-escape", true),
            ("ctrl-cmd-f", true),
            ("ctrl-cmd-i", true),
            ("cmd-i", false),
        ] {
            assert_eq!(
                is_viewer_shortcut(&Keystroke::parse(key).unwrap()),
                local,
                "{key}"
            );
        }
    }
}
