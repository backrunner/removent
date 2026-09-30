use super::*;

/// Reserve only explicit local shortcuts; bare Escape and Cmd-F belong to
/// the remote application. Suppress both key-down and key-up forwarding.
pub(super) fn is_viewer_shortcut(key: &Keystroke) -> bool {
    key.modifiers.control
        && key.modifiers.platform
        && matches!(key.key.as_str(), "escape" | "f" | "i")
}

pub(super) fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

pub(super) fn format_ms(duration: Option<Duration>) -> String {
    duration
        .map(|value| format!("{:.1} ms", value.as_secs_f64() * 1000.))
        .unwrap_or_else(|| "—".into())
}

/// gpui modifiers → wire bitmask.
pub(super) fn key_modifiers(modifiers: &gpui::Modifiers, sent: KeyModifiers) -> KeyModifiers {
    // GPUI Keystroke modifiers omit Caps Lock; its state arrives separately.
    gpui_modifiers(modifiers) | (sent & KeyModifiers::CAPS_LOCK)
}

pub(super) fn gpui_modifiers(m: &gpui::Modifiers) -> KeyModifiers {
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
pub(super) fn mac_vk_of_key(key: &str) -> Option<u16> {
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
