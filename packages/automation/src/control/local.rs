//! Held-state only, never text or an event history; kept within the host process.
use removent_proto::KeyModifiers;

#[derive(Clone, Copy, Default)]
pub(crate) struct LocalHeld {
    keys: [u64; 4],
    buttons: u32,
}
impl LocalHeld {
    pub fn set_key(&mut self, code: u16, down: bool) {
        if code == 0x39 {
            return;
        } // Caps Lock is latched, not held.
        if let Some(word) = self.keys.get_mut(usize::from(code / 64)) {
            let bit = 1u64 << (code % 64);
            if down {
                *word |= bit;
            } else {
                *word &= !bit;
            }
        }
    }
    pub fn key(&self, code: u16) -> bool {
        self.keys
            .get(usize::from(code / 64))
            .is_some_and(|word| word & (1u64 << (code % 64)) != 0)
    }
    pub fn set_button(&mut self, button: u8, down: bool) {
        if button < 32 {
            if down {
                self.buttons |= 1u32 << button;
            } else {
                self.buttons &= !(1u32 << button);
            }
        }
    }
    pub fn button(&self, button: u8) -> bool {
        button < 32 && self.buttons & (1u32 << button) != 0
    }
    pub fn held(&self) -> bool {
        self.buttons != 0 || self.keys.iter().any(|keys| *keys != 0)
    }
    pub fn modifiers(&self) -> KeyModifiers {
        let mut mods = KeyModifiers::empty();
        for key in [0x37, 0x36, 0x38, 0x3c, 0x3a, 0x3d, 0x3b, 0x3e, 0x3f] {
            if self.key(key) {
                mods |= removent_input::modifier_of_vk(key).unwrap();
            }
        }
        mods
    }
}
