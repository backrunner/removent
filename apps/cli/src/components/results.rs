use super::{Card, Tone, badge, dimensions, paint, print_card};
use rust_i18n::t;
use serde_json::Value;

pub(crate) fn label(key: &str) -> String {
    let key = format!("terminal.field.{key}");
    t!(&key).into_owned()
}

pub(crate) fn setting_label(key: &str) -> String {
    let translation = format!("tui.setting.{key}");
    let text = t!(&translation);
    if text == translation {
        key.to_owned()
    } else {
        text.into_owned()
    }
}

pub(crate) fn choice_label(value: &str) -> String {
    let key = format!("tui.value.{}", value.to_lowercase());
    let text = t!(&key);
    if text == key {
        value.to_owned()
    } else {
        text.into_owned()
    }
}

pub(crate) fn setting_value(view: &Value) -> String {
    if view["type"] == "boolean" {
        return format!(
            "[ {} ]",
            if view["value"] == true {
                t!("tui.on")
            } else {
                t!("tui.off")
            }
        );
    }
    if view["type"] == "enum" {
        return choice_label(view["value"].as_str().unwrap_or_default());
    }
    crate::settings::display_value(view)
}

pub(crate) fn setting_group(key: &str) -> String {
    let group = if key.starts_with("authentication.") || matches!(key, "admission" | "paired_only")
    {
        "security"
    } else if key.starts_with("discovery.") {
        "discovery"
    } else if key.starts_with("update_") {
        "updates"
    } else if key.starts_with("host_") || key.starts_with("vnc_") || key == "window_server_capture"
    {
        "sharing"
    } else {
        "general"
    };
    let key = format!("tui.group.{group}");
    t!(&key).into_owned()
}

fn value(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Bool(true) => t!("terminal.yes").into(),
        Value::Bool(false) => t!("terminal.no").into(),
        Value::Null => t!("terminal.none").into(),
        _ => value.to_string(),
    }
}

fn fields(card: &mut Card, data: &Value) {
    if let Some(fields) = data.as_object() {
        for (key, data) in fields {
            if key == "type" || data.is_null() {
                continue;
            }
            let title = label(key);
            if let Some(items) = data.as_array() {
                for item in items {
                    self::fields(card, item);
                }
            } else {
                card.field(&title, &value(data), Tone::Plain);
            }
        }
    }
}

fn status(data: &Value) {
    let mut card = Card::new(&t!("terminal.status"), dimensions().0, Tone::Accent);
    if let Some(name) = data["device_name"].as_str() {
        card.text(name, Tone::Accent);
    }
    let enabled = data["running"] == true;
    let ready = data["host_ready"] == true;
    let sharing = if enabled {
        t!("terminal.action.enable")
    } else {
        t!("terminal.action.disable")
    };
    let readiness = if ready {
        t!("tui.ready")
    } else {
        t!("tui.not_ready")
    };
    let badges = format!(
        "{}  {}",
        badge(&sharing, if enabled { Tone::Success } else { Tone::Muted }),
        badge(
            &readiness,
            if ready { Tone::Success } else { Tone::Warning }
        )
    );
    if console::measure_text_width(&badges) <= card.inner {
        card.line(badges);
    } else {
        card.line(badge(&sharing, Tone::Plain));
        card.line(badge(&readiness, Tone::Plain));
    }
    card.line("");
    for key in ["port", "fp_short", "relay_connected"] {
        if !data[key].is_null() {
            card.field(&label(key), &value(&data[key]), Tone::Plain);
        }
    }
    for key in ["prevent_idle_sleep", "idle_sleep_prevented"] {
        if let Some(on) = data[key].as_bool() {
            card.field(
                &label(key),
                &value(&serde_json::Value::Bool(on)),
                Tone::Plain,
            );
        }
    }
    for key in ["host_error", "relay_error", "power_error"] {
        if let Some(error) = data[key].as_str() {
            card.text(error, Tone::Danger);
        }
    }
    if enabled && !ready {
        card.text(&t!("terminal.not_ready"), Tone::Warning);
    }
    card.divider();
    card.text(&t!("terminal.permissions"), Tone::Accent);
    for key in ["screen_recording_granted", "accessibility_granted"] {
        if let Some(granted) = data[key].as_bool() {
            card.field(
                &label(key),
                &format!(
                    "[ {} ]",
                    if granted {
                        t!("tui.granted")
                    } else {
                        t!("tui.required")
                    }
                ),
                if granted {
                    Tone::Success
                } else {
                    Tone::Warning
                },
            );
        }
    }
    if data["screen_recording_granted"] == false || data["accessibility_granted"] == false {
        card.text(&t!("tui.permissions_action"), Tone::Warning);
    }
    if let Some(sessions) = data["sessions"].as_array() {
        card.divider();
        card.text(
            &t!("terminal.sessions", count = sessions.len()),
            Tone::Accent,
        );
        if sessions.is_empty() {
            card.text(&t!("terminal.no_sessions"), Tone::Muted);
        }
        for session in sessions {
            fields(&mut card, session);
        }
    }
    print_card(card);
    if data["screen_recording_granted"] == false || data["accessibility_granted"] == false {
        println!("  {}", crate::output::command("daemon permissions"));
    }
}

pub(crate) fn render(action: &str, data: &Value) {
    let kind = data["type"].as_str().unwrap_or(action);
    if kind == "status" {
        return status(data);
    }
    let title = match kind {
        "identity" => t!("terminal.identity").into_owned(),
        "pairing_code" | "pairing_pin" => t!("terminal.pairing").into_owned(),
        "pairing_done" => t!("pairing.done").into_owned(),
        "pairing_cleared" => t!("terminal.cleared").into_owned(),
        "ok" => {
            let key = format!("terminal.action.{action}");
            let text = t!(&key);
            if text == key {
                t!("terminal.done").into_owned()
            } else {
                text.into_owned()
            }
        }
        _ => t!("terminal.status").into_owned(),
    };
    let mut card = Card::new(
        &title,
        dimensions().0,
        if kind == "ok" || kind == "pairing_done" {
            Tone::Success
        } else {
            Tone::Accent
        },
    );
    match kind {
        "pairing_code" | "pairing_pin" => {
            let code = data["code"]
                .as_str()
                .or(data["pin"].as_str())
                .unwrap_or("—");
            let code = crate::terminal::clean(code);
            let grouped = code
                .chars()
                .collect::<Vec<_>>()
                .chunks(3)
                .map(|chunk| chunk.iter().collect::<String>())
                .collect::<Vec<_>>()
                .join(" ");
            card.text(&grouped, Tone::Accent);
            card.line("");
            if let Some(expires) = data["expires_at_unix"].as_u64() {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                card.text(
                    &t!("terminal.expires", seconds = expires.saturating_sub(now)),
                    Tone::Warning,
                );
            } else {
                card.text(&t!("terminal.expiry_unknown"), Tone::Muted);
            }
        }
        "ok" => card.text(&t!("tui.completed"), Tone::Success),
        "pairing_cleared" => card.text(&t!("terminal.waiting_code"), Tone::Muted),
        _ => fields(&mut card, data),
    }
    print_card(card);
}

pub(crate) fn welcome(name: &str) {
    println!(
        "\n  {}  {}",
        paint("REMOVENT", Tone::Accent),
        paint(env!("CARGO_PKG_VERSION"), Tone::Muted)
    );
    println!("  {}\n", paint(crate::terminal::clean(name), Tone::Muted));
}
