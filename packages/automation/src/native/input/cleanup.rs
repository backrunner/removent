//! Release only the agent's ownership; a human may now hold the same key/button.
use crate::control::LocalHeld;
use removent_proto::{KeyModifiers, MouseKind};

pub struct Plan {
    pub key: Option<(u16, Option<char>)>,
    pub button: Option<MouseKind>,
    pub flags: KeyModifiers,
    pub modifier_releases: Vec<(u16, KeyModifiers)>,
}
pub fn button_index(up: MouseKind) -> Option<u8> {
    match up {
        MouseKind::LeftUp => Some(0),
        MouseKind::RightUp => Some(1),
        MouseKind::MiddleUp => Some(2),
        _ => None,
    }
}
pub fn native_key(vk: u16) -> u16 {
    if vk == removent_input::UNICODE_ONLY_VK {
        0
    } else {
        vk
    }
}
pub fn plan(
    key: Option<(u16, Option<char>)>,
    button: Option<MouseKind>,
    owned: KeyModifiers,
    local: LocalHeld,
) -> Plan {
    let physical = local.modifiers();
    let flags = owned | physical;
    let mut remaining = flags;
    let mut modifier_releases = Vec::new();
    for (vk, flag) in [
        (0x37, KeyModifiers::COMMAND),
        (0x38, KeyModifiers::SHIFT),
        (0x3a, KeyModifiers::OPTION),
        (0x3b, KeyModifiers::CONTROL),
    ] {
        if owned.contains(flag) && !physical.contains(flag) {
            remaining.remove(flag);
            modifier_releases.push((vk, remaining));
        }
    }
    Plan {
        key: key.filter(|(vk, _)| !local.key(native_key(*vk))),
        button: button.filter(|up| button_index(*up).is_none_or(|index| !local.button(index))),
        flags,
        modifier_releases,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn takeover_keeps_physical_keys_buttons_and_opposite_side_modifiers_held() {
        let mut local = LocalHeld::default();
        local.set_key(0, true);
        local.set_key(0x3c, true);
        local.set_button(0, true);
        let release = plan(
            Some((0, None)),
            Some(MouseKind::LeftUp),
            KeyModifiers::SHIFT | KeyModifiers::COMMAND,
            local,
        );
        assert!(release.key.is_none());
        assert!(release.button.is_none());
        assert_eq!(release.modifier_releases, vec![(0x37, KeyModifiers::SHIFT)]);
        assert!(release.flags.contains(KeyModifiers::SHIFT));
        let ordinary = plan(
            Some((1, None)),
            Some(MouseKind::RightUp),
            KeyModifiers::COMMAND,
            local,
        );
        assert_eq!(ordinary.key, Some((1, None)));
        assert_eq!(ordinary.button, Some(MouseKind::RightUp));
        assert_eq!(ordinary.flags, KeyModifiers::COMMAND | KeyModifiers::SHIFT);
    }
    #[test]
    fn cancelled_unicode_keeps_its_payload_for_a_real_matching_key_up() {
        let release = plan(
            Some((removent_input::UNICODE_ONLY_VK, Some('中'))),
            None,
            KeyModifiers::empty(),
            LocalHeld::default(),
        );
        assert_eq!(
            release.key,
            Some((removent_input::UNICODE_ONLY_VK, Some('中')))
        );
        let mut local = LocalHeld::default();
        local.set_key(0, true);
        assert!(
            plan(release.key, None, KeyModifiers::empty(), local)
                .key
                .is_none()
        );
    }
}
