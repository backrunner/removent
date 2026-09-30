use super::*;

impl ViewerView {
    /// Map a window point to remote frame pixels, accounting for the title bar and
    /// the ObjectFit::Contain letterboxing. `clamp` selects out-of-bounds handling:
    /// false → None (letterbox/title-bar points must not be sent), true → clamped to
    /// the frame edge (button releases may land outside the picture and the host must
    /// still see them, or the button stays pressed remotely).
    ///
    /// Note: gpui exposes no way to read the CGEvent source user-data of an incoming
    /// event, so two Macs controlling each other cannot detect re-injected events
    /// here (loop protection would have to live at the CGEventTap level).
    pub(super) fn frame_px(
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
    pub(super) fn forward_mouse(
        &mut self,
        pos: Point<gpui::Pixels>,
        kind: MouseKind,
        window: &Window,
    ) {
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
    pub(super) fn forward_key(&mut self, keystroke: &Keystroke, kind: KeyKind) {
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
    pub(super) fn release_held_inputs(&mut self) {
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
    pub(super) fn arm_toolbar_hide_timer(&mut self, cx: &mut Context<Self>) {
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
    pub(super) fn forward_modifiers(&mut self, ev: &ModifiersChangedEvent) {
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
}
