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
}
impl KeyboardInjector {
    pub fn inject(
        &mut self,
        vk: u16,
        mods: KeyModifiers,
        kind: KeyKind,
        text: Option<char>,
    ) -> Result<(), String> {
        self.modifiers = mods;
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
    _phase: ScrollPhase,
    _mods: KeyModifiers,
) -> Result<(), String> {
    crate::portable::with(|e| {
        if dx != 0. {
            e.scroll((-dx / 16.).round() as i32, Axis::Horizontal)?;
        }
        if dy != 0. {
            e.scroll((-dy / 16.).round() as i32, Axis::Vertical)?;
        }
        Ok(())
    })
}

pub fn key_text(vk: u16, mods: KeyModifiers, unicode: Option<char>) -> Option<char> {
    if mods.intersects(KeyModifiers::COMMAND | KeyModifiers::CONTROL)
        || matches!(vk, 0x24 | 0x30 | 0x33 | 0x35 | 0x4C | 0x72..=0x7E)
    {
        return None;
    }
    unicode.filter(|c| !c.is_control() && !('\u{f700}'..='\u{f8ff}').contains(c))
}
