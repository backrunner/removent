use enigo::{Axis, Direction, Key, Keyboard, Mouse};
use removent_proto::{KeyKind, KeyModifiers, ScrollPhase};
pub const UNICODE_ONLY_VK: u16 = 0xffff;
pub fn modifier_of_vk(vk_code: u16) -> Option<KeyModifiers> {
    // macOS virtual key codes; mirrors modifier_flag_of_vk.
    match vk_code {
        0x37 | 0x36 => Some(KeyModifiers::COMMAND), // L/R Command
        0x38 | 0x3C => Some(KeyModifiers::SHIFT),   // L/R Shift
        0x3A | 0x3D => Some(KeyModifiers::OPTION),  // L/R Option
        0x3B | 0x3E => Some(KeyModifiers::CONTROL), // L/R Control
        0x39 => Some(KeyModifiers::CAPS_LOCK),      // Caps Lock
        0x3F => Some(KeyModifiers::FUNCTION),       // Fn
        _ => None,
    }
}
fn key(vk: u16) -> Option<Key> {
    Some(match vk {
        0x24 | 0x4c => Key::Return,
        0x30 => Key::Tab,
        0x31 => Key::Space,
        0x33 => Key::Backspace,
        0x35 => Key::Escape,
        0x37 | 0x36 => Key::Meta,
        0x38 | 0x3c => Key::Shift,
        0x3a | 0x3d => Key::Alt,
        0x3b | 0x3e => Key::Control,
        0x39 => Key::CapsLock,
        0x73 => Key::Home,
        0x77 => Key::End,
        0x74 => Key::PageUp,
        0x79 => Key::PageDown,
        0x75 => Key::Delete,
        0x7b => Key::LeftArrow,
        0x7c => Key::RightArrow,
        0x7d => Key::DownArrow,
        0x7e => Key::UpArrow,
        0x7a => Key::F1,
        0x78 => Key::F2,
        0x63 => Key::F3,
        0x76 => Key::F4,
        0x60 => Key::F5,
        0x61 => Key::F6,
        0x62 => Key::F7,
        0x64 => Key::F8,
        0x65 => Key::F9,
        0x6d => Key::F10,
        0x67 => Key::F11,
        0x6f => Key::F12,
        0x69 => Key::F13,
        0x6b => Key::F14,
        0x71 => Key::F15,
        0x6a => Key::F16,
        0x40 => Key::F17,
        0x4f => Key::F18,
        0x50 => Key::F19,
        0x5a => Key::F20,
        0x41 => Key::Unicode('.'),
        0x43 => Key::Unicode('*'),
        0x45 => Key::Unicode('+'),
        0x4b => Key::Unicode('/'),
        0x4e => Key::Unicode('-'),
        0x51 => Key::Unicode('='),
        0x52..=0x59 => Key::Unicode(char::from(b'0' + (vk - 0x52) as u8)),
        0x5b => Key::Unicode('8'),
        0x5c => Key::Unicode('9'),
        _ => {
            let keys = "asdfhgzxcv\0bqwerty123465=97-80]ou[ip\0lj'k;\\,/nm.\0 \x60";
            Key::Unicode(keys.chars().nth(vk as usize).filter(|c| *c != '\0')?)
        }
    })
}
#[derive(Default)]
pub struct KeyboardInjector {
    pub modifiers: KeyModifiers,
    pressed: std::collections::HashMap<u16, Key>,
    #[cfg(windows)]
    committed_text: std::collections::HashSet<u16>,
}
impl KeyboardInjector {
    pub fn inject(
        &mut self,
        vk: u16,
        mods: KeyModifiers,
        kind: KeyKind,
        text: Option<char>,
    ) -> Result<(), String> {
        let previous = self.modifiers;
        self.modifiers = mods;
        // Committed IME text is an insertion, with no physical key to release.
        // Enigo's Windows Unicode fallback inserts text even for Key::Release,
        // which would duplicate characters if called for both wire transitions.
        #[cfg(windows)]
        if vk == UNICODE_ONLY_VK {
            return if kind == KeyKind::Down {
                match text.filter(|c| !c.is_control()) {
                    Some(ch) => crate::portable::with(|e| e.text(&ch.to_string())),
                    None => Ok(()),
                }
            } else {
                Ok(())
            };
        }
        #[cfg(windows)]
        {
            if kind == KeyKind::Up && self.committed_text.remove(&vk) {
                return Ok(());
            }
            if kind == KeyKind::Down
                && !mods.intersects(KeyModifiers::CONTROL | KeyModifiers::COMMAND)
                && let Some(ch) = text.filter(|c| !c.is_ascii() && !c.is_control())
            {
                crate::portable::with(|e| e.text(&ch.to_string()))?;
                self.committed_text.insert(vk);
                return Ok(());
            }
        }
        if vk == 0x39 && kind == KeyKind::FlagsChanged {
            if previous.contains(KeyModifiers::CAPS_LOCK) != mods.contains(KeyModifiers::CAPS_LOCK)
            {
                crate::portable::with(|e| e.key(Key::CapsLock, Direction::Click))?;
            }
            return Ok(());
        }
        let down = match kind {
            KeyKind::Down => true,
            KeyKind::Up => false,
            KeyKind::FlagsChanged => modifier_of_vk(vk).is_some_and(|m| mods.contains(m)),
        };
        let mapped = if !down {
            self.pressed.remove(&vk).or_else(|| key(vk))
        } else if !mods.intersects(KeyModifiers::CONTROL | KeyModifiers::COMMAND)
            && text.is_some_and(|c| !c.is_control())
        {
            text.map(Key::Unicode)
        } else {
            key(vk)
        };
        let Some(mapped) = mapped else {
            return Err(format!("Unsupported key code {vk:#x}"));
        };
        crate::portable::with(|e| {
            e.key(
                mapped,
                if down {
                    Direction::Press
                } else {
                    Direction::Release
                },
            )
        })?;
        if down {
            self.pressed.insert(vk, mapped);
        }
        Ok(())
    }
}
pub fn inject_key(
    vk: u16,
    mods: KeyModifiers,
    kind: KeyKind,
    text: Option<char>,
) -> Result<(), String> {
    KeyboardInjector::default().inject(vk, mods, kind, text)
}
pub fn build_key(vk: u16, mods: KeyModifiers, kind: KeyKind) -> Result<(), String> {
    inject_key(vk, mods, kind, None)
}
pub fn inject_scroll_pixels(dx: f64, dy: f64, phase: ScrollPhase) -> Result<(), String> {
    inject_scroll_with_modifiers(dx, dy, phase, KeyModifiers::empty())
}
pub fn inject_scroll_with_modifiers(
    dx: f64,
    dy: f64,
    phase: ScrollPhase,
    _mods: KeyModifiers,
) -> Result<(), String> {
    ScrollInjector::default().inject(dx, dy, phase)
}

/// Preserve sub-wheel deltas for a session instead of dropping trackpad motion.
#[derive(Default)]
pub struct ScrollInjector {
    residual: [f64; 2],
}
impl ScrollInjector {
    fn steps(&mut self, dx: f64, dy: f64, phase: ScrollPhase) -> [i32; 2] {
        if phase == ScrollPhase::Began {
            self.residual = [0.; 2];
        }
        let mut steps = [0; 2];
        for (i, delta) in [dx, dy].into_iter().enumerate() {
            self.residual[i] -= delta / 16.;
            steps[i] = self.residual[i].trunc() as i32;
            self.residual[i] -= steps[i] as f64;
        }
        if phase == ScrollPhase::Ended {
            self.residual = [0.; 2];
        }
        steps
    }
    pub fn inject(&mut self, dx: f64, dy: f64, phase: ScrollPhase) -> Result<(), String> {
        let [horizontal, vertical] = self.steps(dx, dy, phase);
        if horizontal == 0 && vertical == 0 {
            return Ok(());
        }
        crate::portable::with(|e| {
            if horizontal != 0 {
                e.scroll(horizontal, Axis::Horizontal)?;
            }
            if vertical != 0 {
                e.scroll(vertical, Axis::Vertical)?;
            }
            Ok(())
        })
    }
}

pub fn key_text(vk: u16, mods: KeyModifiers, unicode: Option<char>) -> Option<char> {
    if mods.intersects(KeyModifiers::COMMAND | KeyModifiers::CONTROL)
        || matches!(vk, 0x24 | 0x30 | 0x33 | 0x35 | 0x4C | 0x72..=0x7E)
    {
        return None;
    }
    unicode.filter(|c| !c.is_control() && !('\u{f700}'..='\u{f8ff}').contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn trackpad_deltas_accumulate_and_reset_between_gestures() {
        let mut scroll = ScrollInjector::default();
        assert_eq!(scroll.steps(0., 4., ScrollPhase::Began), [0, 0]);
        for _ in 0..2 {
            assert_eq!(scroll.steps(0., 4., ScrollPhase::Changed), [0, 0]);
        }
        assert_eq!(scroll.steps(0., 4., ScrollPhase::Changed), [0, -1]);
        assert_eq!(scroll.steps(0., 12., ScrollPhase::Ended), [0, 0]);
        assert_eq!(scroll.steps(0., 4., ScrollPhase::Began), [0, 0]);
        assert_eq!(scroll.steps(-32., -20., ScrollPhase::Changed), [2, 1]);
    }
    #[test]
    fn canonical_wire_keys_cover_shortcuts_and_keypad() {
        assert_eq!(key(0), Some(Key::Unicode('a')));
        assert_eq!(key(8), Some(Key::Unicode('c')));
        assert_eq!(key(9), Some(Key::Unicode('v')));
        assert_eq!(key(0x3b), Some(Key::Control));
        assert_eq!(key(0x52), Some(Key::Unicode('0')));
        assert_eq!(key(0x59), Some(Key::Unicode('7')));
        assert_eq!(key(0x5c), Some(Key::Unicode('9')));
        assert_eq!(key(0xffff), None);
    }
}
