use super::*;
use removent_input::{keyboard::KeyboardInjector, mouse::MouseInjector};
use removent_proto::automation::Button;
use removent_proto::{KeyKind, KeyModifiers, MouseKind, ScrollPhase};
mod cleanup;

struct Gesture {
    keyboard: KeyboardInjector,
    mouse: MouseInjector,
    key: Option<(u16, Option<char>)>,
    button: Option<MouseKind>,
    point: (f64, f64),
    scroll: bool,
    control: crate::control::Control,
}
impl Gesture {
    fn modifiers(&mut self, mods: KeyModifiers) -> Result<(), Failure> {
        for (vk, flag) in [
            (0x37, KeyModifiers::COMMAND),
            (0x38, KeyModifiers::SHIFT),
            (0x3a, KeyModifiers::OPTION),
            (0x3b, KeyModifiers::CONTROL),
        ] {
            if self.keyboard.modifiers.contains(flag) != mods.contains(flag) {
                let next = if mods.contains(flag) {
                    self.keyboard.modifiers | flag
                } else {
                    self.keyboard.modifiers & !flag
                };
                self.keyboard
                    .inject(vk, next, KeyKind::FlagsChanged, None)
                    .map_err(failure)?;
            }
        }
        Ok(())
    }
    fn mouse(&mut self, kind: MouseKind, p: (f64, f64)) -> Result<(), Failure> {
        self.point = p;
        let local = self.control.local_state();
        if cleanup::button_index(kind).is_some_and(|index| local.button(index)) {
            return Ok(());
        }
        let flags = if cleanup::button_index(kind).is_some() {
            self.keyboard.modifiers | local.modifiers()
        } else {
            self.keyboard.modifiers
        };
        self.mouse.inject(kind, p.0, p.1, flags).map_err(failure)
    }
    fn key(
        &mut self,
        vk: u16,
        text: Option<char>,
        hold: u32,
        cancel: &CancellationToken,
    ) -> Result<(), Failure> {
        check_cancel(cancel)?;
        self.key = Some((vk, text));
        self.keyboard
            .inject(vk, self.keyboard.modifiers, KeyKind::Down, text)
            .map_err(failure)?;
        pause(hold, cancel)?;
        let local = self.control.local_state();
        if !local.key(cleanup::native_key(vk)) {
            self.keyboard
                .inject(
                    vk,
                    self.keyboard.modifiers | local.modifiers(),
                    KeyKind::Up,
                    text,
                )
                .map_err(failure)?;
        }
        self.key = None;
        Ok(())
    }
}
impl Drop for Gesture {
    fn drop(&mut self) {
        let plan = cleanup::plan(
            self.key.take(),
            self.button.take(),
            self.keyboard.modifiers,
            self.control.local_state(),
        );
        if let Some((vk, text)) = plan.key {
            let _ = self.keyboard.inject(vk, plan.flags, KeyKind::Up, text);
        }
        if let Some(up) = plan.button {
            let _ = self
                .mouse
                .inject(up, self.point.0, self.point.1, plan.flags);
        }
        if self.scroll {
            let _ = removent_input::inject_scroll_pixels(0., 0., ScrollPhase::Ended);
        }
        for (vk, flags) in plan.modifier_releases {
            let _ = self.keyboard.inject(vk, flags, KeyKind::FlagsChanged, None);
        }
    }
}
fn pause(ms: u32, cancel: &CancellationToken) -> Result<(), Failure> {
    let end = Instant::now() + Duration::from_millis(ms as u64);
    loop {
        check_cancel(cancel)?;
        let now = Instant::now();
        if now >= end {
            return Ok(());
        }
        std::thread::sleep((end - now).min(Duration::from_millis(10)));
    }
}
fn modifier(name: &str) -> Option<KeyModifiers> {
    match name {
        "meta" | "cmd" | "command" => Some(KeyModifiers::COMMAND),
        "ctrl" | "control" => Some(KeyModifiers::CONTROL),
        "alt" | "option" => Some(KeyModifiers::OPTION),
        "shift" => Some(KeyModifiers::SHIFT),
        _ => None,
    }
}
fn key_code(key: &str) -> Option<u16> {
    let named = match key {
        "enter" | "return" => 0x24,
        "tab" => 0x30,
        "space" => 0x31,
        "backspace" => 0x33,
        "escape" | "esc" => 0x35,
        "delete" => 0x75,
        "home" => 0x73,
        "end" => 0x77,
        "pageup" => 0x74,
        "pagedown" => 0x79,
        "left" | "arrowleft" => 0x7b,
        "right" | "arrowright" => 0x7c,
        "down" | "arrowdown" => 0x7d,
        "up" | "arrowup" => 0x7e,
        "f1" => 0x7a,
        "f2" => 0x78,
        "f3" => 0x63,
        "f4" => 0x76,
        "f5" => 0x60,
        "f6" => 0x61,
        "f7" => 0x62,
        "f8" => 0x64,
        "f9" => 0x65,
        "f10" => 0x6d,
        "f11" => 0x67,
        "f12" => 0x6f,
        _ => {
            return [
                ("a", 0),
                ("s", 1),
                ("d", 2),
                ("f", 3),
                ("h", 4),
                ("g", 5),
                ("z", 6),
                ("x", 7),
                ("c", 8),
                ("v", 9),
                ("b", 11),
                ("q", 12),
                ("w", 13),
                ("e", 14),
                ("r", 15),
                ("y", 16),
                ("t", 17),
                ("1", 18),
                ("2", 19),
                ("3", 20),
                ("4", 21),
                ("6", 22),
                ("5", 23),
                ("=", 24),
                ("9", 25),
                ("7", 26),
                ("-", 27),
                ("8", 28),
                ("0", 29),
                ("]", 30),
                ("o", 31),
                ("u", 32),
                ("[", 33),
                ("i", 34),
                ("p", 35),
                ("l", 37),
                ("j", 38),
                ("'", 39),
                ("k", 40),
                (";", 41),
                ("\\", 42),
                (",", 43),
                ("/", 44),
                ("n", 45),
                ("m", 46),
                (".", 47),
                ("`", 50),
            ]
            .into_iter()
            .find(|(s, _)| *s == key)
            .map(|(_, vk)| vk);
        }
    };
    Some(named)
}
fn chord(key: &str) -> Result<(KeyModifiers, u16), Failure> {
    let normalized = key.to_ascii_lowercase();
    let mut parts = normalized.split('+').collect::<Vec<_>>();
    let key = parts.pop().unwrap_or_default();
    let mut mods = KeyModifiers::empty();
    for part in parts {
        mods |= modifier(part)
            .ok_or_else(|| Failure::new("invalid_argument", "Unknown key modifier"))?;
    }
    Ok((
        mods,
        key_code(key).ok_or_else(|| {
            Failure::new("invalid_argument", "Unknown key; use type_text for Unicode")
        })?,
    ))
}
pub fn execute(
    command: &Command,
    screen: &screen::Screen,
    cancel: &CancellationToken,
    control: &crate::control::Control,
) -> Outcome {
    let mut g = Gesture {
        control: control.clone(),
        keyboard: Default::default(),
        mouse: Default::default(),
        key: None,
        button: None,
        point: (0., 0.),
        scroll: false,
    };
    match command {
        Command::Click {
            point,
            button,
            count,
            modifiers,
            hold_ms,
        } => {
            let p = screen.point(*point)?;
            let mut mods = KeyModifiers::empty();
            for m in modifiers {
                mods |= modifier(m).unwrap();
            }
            g.modifiers(mods)?;
            let (down, up) = match button {
                Button::Left => (MouseKind::LeftDown, MouseKind::LeftUp),
                Button::Right => (MouseKind::RightDown, MouseKind::RightUp),
                Button::Middle => (MouseKind::MiddleDown, MouseKind::MiddleUp),
            };
            for _ in 0..*count {
                check_cancel(cancel)?;
                g.button = Some(up);
                g.mouse(down, p)?;
                pause(*hold_ms, cancel)?;
                g.mouse(up, p)?;
                g.button = None;
            }
        }
        Command::Move { point } => g.mouse(MouseKind::Moved, screen.point(*point)?)?,
        Command::Drag {
            from,
            to,
            duration_ms,
        } => {
            let a = screen.point(*from)?;
            let b = screen.point(*to)?;
            g.button = Some(MouseKind::LeftUp);
            g.mouse(MouseKind::LeftDown, a)?;
            let steps = (*duration_ms / 16).max(1);
            for step in 1..=steps {
                pause(*duration_ms / steps, cancel)?;
                let t = step as f64 / steps as f64;
                g.mouse(
                    MouseKind::LeftDragged,
                    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t),
                )?;
            }
            g.mouse(MouseKind::LeftUp, b)?;
            g.button = None;
        }
        Command::Scroll { point, dx, dy } => {
            g.mouse(MouseKind::Moved, screen.point(*point)?)?;
            g.scroll = true;
            // API uses viewport pixels: positive y scrolls content down, matching browser wheel events.
            // The shared injector already converts viewport deltas to CG's sign.
            removent_input::inject_scroll_pixels(*dx, *dy, ScrollPhase::Began).map_err(failure)?;
            removent_input::inject_scroll_pixels(0., 0., ScrollPhase::Ended).map_err(failure)?;
            g.scroll = false;
        }
        Command::PressKey { key, hold_ms } => {
            let (mods, vk) = chord(key)?;
            g.modifiers(mods)?;
            g.key(vk, None, *hold_ms, cancel)?;
        }
        Command::TypeText { text } => {
            if text
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
            {
                return Err(Failure::new(
                    "invalid_argument",
                    "Text contains unsupported control characters",
                ));
            }
            for c in text.chars() {
                let (vk, text) = match c {
                    '\n' | '\r' => (0x24, None),
                    '\t' => (0x30, None),
                    _ => (removent_input::UNICODE_ONLY_VK, Some(c)),
                };
                g.key(vk, text, 0, cancel)?;
            }
        }
        _ => return Err(Failure::new("unsupported", "Unsupported input action")),
    }
    executed()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chords_validate_before_injecting() {
        assert_eq!(
            chord("Meta+Shift+A").unwrap(),
            (KeyModifiers::COMMAND | KeyModifiers::SHIFT, 0)
        );
        assert!(chord("meta+wat").is_err());
        assert!(chord("unknown+a").is_err());
    }
}
