//! Scrollback-friendly presentation. No alternate screen, raw-mode ownership or
//! terminal control sequences in redirected output. JSON keeps its wire shape.
use rust_i18n::t;
use serde_json::Value;
use std::io::{IsTerminal, Write};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Format {
    #[default]
    Auto,
    Human,
    Json,
}

impl Format {
    // Only historically JSON commands opt into automatic JSON when redirected.
    pub fn json(self, legacy_json: bool) -> bool {
        self == Self::Json
            || (self == Self::Auto && legacy_json && !std::io::stdout().is_terminal())
    }
}

pub(super) fn clean(value: &str) -> String {
    value
        .chars()
        .filter(|c| {
            !c.is_control() && !matches!(*c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .collect()
}

pub(super) fn width() -> usize {
    if std::io::stdout().is_terminal() {
        usize::from(console::Term::stdout().size().1).max(4)
    } else {
        100
    }
}

fn closing(c: char) -> bool {
    "，。、：；？！）】》」』”’、,.:;!?)]}".contains(c)
}
fn opening(c: char) -> bool {
    "（【《「『“‘([{".contains(c)
}

/// Shared wrapping for all result fields, including CJK punctuation boundaries.
/// Values are never elided: fingerprints, paths and recovery commands stay copyable.
pub(super) fn wrap(value: &str, columns: usize) -> Vec<String> {
    let chars: Vec<char> = clean(value).chars().collect();
    let mut lines = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let mut end = start;
        let mut used = 0;
        while end < chars.len() {
            let size = console::measure_text_width(&chars[end].to_string());
            if used + size > columns && end > start {
                break;
            }
            used += size;
            end += 1;
        }
        let original = end;
        while end > start + 1
            && (opening(chars[end - 1]) || (end < chars.len() && closing(chars[end])))
        {
            end -= 1;
        }
        // A punctuation cluster wider than the terminal is kept together.
        if end == start + 1 && (opening(chars[start]) || (end < chars.len() && closing(chars[end])))
        {
            end = original;
            while end < chars.len() && (closing(chars[end]) || opening(chars[end - 1])) {
                end += 1;
            }
        }
        lines.push(chars[start..end].iter().collect());
        start = end;
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

pub(super) fn note(value: &str) {
    for line in wrap(value, width().saturating_sub(2).max(2)) {
        println!("  {line}");
    }
}

pub(super) fn heading(value: &str) {
    let value = clean(value);
    let styled = console::Style::new().bold().for_stdout();
    for line in wrap(&value, width().saturating_sub(2).max(2)) {
        if std::io::stdout().is_terminal()
            && std::env::var_os("NO_COLOR").is_none()
            && std::env::var("TERM").as_deref() != Ok("dumb")
        {
            println!("\n  {}", styled.apply_to(line));
        } else {
            println!("\n  {line}");
        }
    }
}

pub(super) fn field(label: &str, value: &str) {
    let label = clean(label);
    let value = clean(value);
    let line = format!("{label}: {value}");
    if console::measure_text_width(&line) + 2 <= width() {
        note(&line);
    } else {
        note(&format!("{label}:"));
        note(&value);
    }
}

fn scalar(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Bool(true) => t!("terminal.yes").into(),
        Value::Bool(false) => t!("terminal.no").into(),
        Value::Null => t!("terminal.none").into(),
        _ => value.to_string(),
    }
}

fn fields(value: &Value) {
    if let Some(object) = value.as_object() {
        for (key, value) in object {
            if key == "type" {
                continue;
            }
            let translation = format!("terminal.field.{key}");
            let label = t!(&translation);
            let label = if label == translation {
                key.as_str()
            } else {
                &label
            };
            match value {
                Value::Array(items) => {
                    field(label, &items.len().to_string());
                    for item in items {
                        fields(item);
                    }
                }
                Value::Object(_) => {
                    note(label);
                    fields(value);
                }
                _ => field(label, &scalar(value)),
            }
        }
    } else {
        note(&scalar(value));
    }
}

pub(super) fn render(action: &str, value: &Value, format: Format) -> anyhow::Result<()> {
    if format.json(true) {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        human(action, value);
    }
    std::io::stdout().flush()?;
    Ok(())
}

pub(super) fn event(action: &str, value: &Value, format: Format) -> anyhow::Result<()> {
    if format.json(true) {
        println!("{}", serde_json::to_string(value)?);
    } else {
        human(action, value);
    }
    std::io::stdout().flush()?;
    Ok(())
}

fn status(value: &Value) {
    heading(&t!("terminal.status"));
    for key in [
        "device_name",
        "running",
        "host_ready",
        "port",
        "fp_short",
        "relay_connected",
        "host_error",
        "relay_error",
        "prevent_idle_sleep",
        "idle_sleep_prevented",
        "power_error",
    ] {
        if !value[key].is_null() {
            fields(&serde_json::json!({key: value[key]}));
        }
    }
    if value["running"] == true && value["host_ready"] == false {
        note(&t!("terminal.not_ready"));
    }
    if value.get("screen_recording_granted").is_some()
        || value.get("accessibility_granted").is_some()
    {
        heading(&t!("terminal.permissions"));
        for key in ["screen_recording_granted", "accessibility_granted"] {
            if !value[key].is_null() {
                fields(&serde_json::json!({key: value[key]}));
            }
        }
        if value["screen_recording_granted"] == false || value["accessibility_granted"] == false {
            note(&t!("terminal.permissions_hint", command = ""));
            println!("  {}", crate::output::command("daemon permissions"));
        }
    }
    if let Some(sessions) = value["sessions"].as_array() {
        heading(&t!("terminal.sessions", count = sessions.len()));
        if sessions.is_empty() {
            note(&t!("terminal.no_sessions"));
        }
        for session in sessions {
            fields(session);
        }
    }
}

fn human(action: &str, value: &Value) {
    if crate::components::enabled() {
        crate::components::results::render(action, value);
        return;
    }
    match value["type"].as_str().unwrap_or(action) {
        "pairing_code" | "pairing_pin" => {
            heading(&t!("terminal.pairing"));
            note(
                value["code"]
                    .as_str()
                    .or(value["pin"].as_str())
                    .unwrap_or("—"),
            );
            if let Some(expires) = value["expires_at_unix"].as_u64() {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                note(&t!(
                    "terminal.expires",
                    seconds = expires.saturating_sub(now)
                ));
            } else {
                note(&t!("terminal.expiry_unknown"));
            }
        }
        "pairing_done" => {
            heading(&t!("pairing.done"));
            fields(value);
        }
        "pairing_cleared" => heading(&t!("terminal.cleared")),
        "ok" => {
            let key = format!("terminal.action.{action}");
            let message = t!(&key);
            heading(&if message == key {
                t!("terminal.done")
            } else {
                message
            });
        }
        "status" => status(value),
        "identity" => {
            heading(&t!("terminal.identity"));
            fields(value);
        }
        _ => {
            heading(&t!("terminal.status"));
            fields(value);
            if value["running"] == true && value["host_ready"] == false {
                note(&t!("terminal.not_ready"));
            }
            if value["screen_recording_granted"] == false || value["accessibility_granted"] == false
            {
                note(&t!("terminal.permissions_hint", command = ""));
                println!("  {}", crate::output::command("daemon permissions"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wraps_cjk_without_orphaning_punctuation_or_losing_text() {
        let value = "共享已开启，监听尚未就绪（请检查权限）。";
        for width in [8, 12, 20, 40] {
            let lines = wrap(value, width);
            assert_eq!(lines.concat(), value);
            for line in lines {
                assert!(!closing(line.chars().next().unwrap()), "{line}");
                assert!(!opening(line.chars().last().unwrap()), "{line}");
                assert!(console::measure_text_width(&line) <= width, "{line}");
            }
        }
    }
    #[test]
    fn untrusted_fields_cannot_control_the_terminal() {
        assert_eq!(clean("Mac\u{1b}[2J\r\n\u{202e}name"), "Mac[2Jname");
        let fingerprint = "0123456789abcdef".repeat(4);
        assert_eq!(wrap(&fingerprint, 20).concat(), fingerprint);
    }
}
