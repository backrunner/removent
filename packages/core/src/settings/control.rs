//! Typed settings catalog and locked edits shared by management clients.
use super::Settings;
use crate::{CoreError, DataDirLock, DataPaths};
use serde_json::{Value, json};

#[derive(Clone, Copy)]
pub enum Kind {
    Bool,
    Integer(u64, u64),
    Text(usize),
    Choice(&'static [&'static str]),
}

#[derive(Clone, Copy)]
pub struct Setting {
    pub key: &'static str,
    pub kind: Kind,
    pub secret: bool,
    /// Desktop preferences need a fresh desktop process; host settings reload
    /// through the daemon and may interrupt existing host sessions.
    pub host: bool,
}

macro_rules! setting {
    ($key:literal, $kind:expr, $secret:literal, $host:literal) => {
        Setting {
            key: $key,
            kind: $kind,
            secret: $secret,
            host: $host,
        }
    };
}

pub const SETTINGS: &[Setting] = &[
    setting!(
        "device_name",
        Kind::Text(super::DEVICE_NAME_MAX_BYTES),
        false,
        true
    ),
    setting!(
        "language",
        Kind::Choice(&["System", "En", "ZhCn"]),
        false,
        false
    ),
    setting!(
        "theme",
        Kind::Choice(&["System", "Light", "Dark"]),
        false,
        false
    ),
    setting!("discovery.removent", Kind::Bool, false, false),
    setting!("discovery.vnc", Kind::Bool, false, false),
    setting!("discovery.rdp", Kind::Bool, false, false),
    setting!(
        "admission",
        Kind::Choice(&["AlwaysAsk", "TrustedAuto", "DenyAll"]),
        false,
        true
    ),
    setting!(
        "authentication.mode",
        Kind::Choice(&["system_account", "pairing_code", "password", "otp", "none"]),
        false,
        true
    ),
    setting!(
        "authentication.pairing_policy",
        Kind::Choice(&["remember_device", "every_connection"]),
        false,
        true
    ),
    setting!("authentication.password", Kind::Text(1024), true, true),
    setting!("authentication.otp_secret", Kind::Text(1024), true, true),
    setting!("host_enabled", Kind::Bool, false, true),
    setting!("prevent_idle_sleep", Kind::Bool, false, true),
    setting!("host_port", Kind::Integer(1, 65535), false, true),
    setting!("window_server_capture", Kind::Bool, false, true),
    setting!("paired_only", Kind::Bool, false, true),
    setting!("vnc_enabled", Kind::Bool, false, true),
    setting!("vnc_port", Kind::Integer(1, 65535), false, true),
    setting!("vnc_password", Kind::Text(1024), true, true),
    setting!("audio_enabled_default", Kind::Bool, false, false),
    setting!(
        "clipboard_clear_after_secs",
        Kind::Integer(0, u32::MAX as u64),
        false,
        false
    ),
    setting!(
        "video_quality",
        Kind::Choice(&["Auto", "Smooth", "Balanced", "HighQuality", "Extreme"]),
        false,
        false
    ),
    setting!("update_check_enabled", Kind::Bool, false, false),
    setting!(
        "update_channel",
        Kind::Choice(&["stable", "beta"]),
        false,
        false
    ),
    setting!("update_endpoint", Kind::Text(2048), false, false),
];

#[derive(Debug, thiserror::Error)]
pub enum EditError {
    #[error("Unknown setting: {0}")]
    Unknown(String),
    #[error("Invalid value for {key}; expected {expected}")]
    Invalid { key: &'static str, expected: String },
    #[error("{0}")]
    Storage(#[from] CoreError),
}

pub fn find(key: &str) -> Result<&'static Setting, EditError> {
    SETTINGS
        .iter()
        .find(|setting| setting.key == key)
        .ok_or_else(|| EditError::Unknown(key.to_owned()))
}

impl Setting {
    pub fn value_type(&self) -> &'static str {
        match self.kind {
            Kind::Bool => "boolean",
            Kind::Integer(..) => "integer",
            Kind::Text(_) => "string",
            Kind::Choice(_) => "enum",
        }
    }

    pub fn accepted(&self) -> String {
        match self.kind {
            Kind::Bool => "true | false".into(),
            Kind::Integer(min, max) => format!("{min}..{max}"),
            Kind::Text(max) => format!("text, at most {max} bytes"),
            Kind::Choice(values) => values.join(" | "),
        }
    }

    fn invalid(&self, expected: impl Into<String>) -> EditError {
        EditError::Invalid {
            key: self.key,
            expected: expected.into(),
        }
    }

    pub fn parse(&self, input: &str) -> Result<Value, EditError> {
        let invalid = || self.invalid(self.accepted());
        match self.kind {
            Kind::Bool => match input.to_ascii_lowercase().as_str() {
                "true" | "on" => Ok(Value::Bool(true)),
                "false" | "off" => Ok(Value::Bool(false)),
                _ => Err(invalid()),
            },
            Kind::Integer(min, max) => input
                .parse::<u64>()
                .ok()
                .filter(|value| (min..=max).contains(value))
                .map(Value::from)
                .ok_or_else(invalid),
            Kind::Text(max) => {
                if input.len() > max || input.chars().any(char::is_control) {
                    return Err(invalid());
                }
                if self.key == "device_name" && input.trim().is_empty() {
                    return Err(self.invalid("a non-empty device name"));
                }
                if self.key == "update_endpoint" && !input.is_empty() {
                    let valid = url::Url::parse(input).is_ok_and(|url| {
                        url.scheme() == "https"
                            && url.host_str().is_some()
                            && url.username().is_empty()
                            && url.password().is_none()
                            && url.fragment().is_none()
                    });
                    if !valid {
                        return Err(self.invalid(
                            "an HTTPS URL without credentials or a fragment, or empty text",
                        ));
                    }
                }
                if self.key == "authentication.otp_secret" && !input.is_empty() {
                    let auth = crate::AuthenticationSettings {
                        otp_secret: input.into(),
                        ..Default::default()
                    };
                    if auth.totp().is_err() {
                        return Err(self.invalid("a valid Base32 OTP secret"));
                    }
                }
                Ok(Value::String(input.into()))
            }
            Kind::Choice(values) => values
                .iter()
                .find(|value| normalize(value) == normalize(input))
                .map(|value| Value::String((*value).into()))
                .ok_or_else(invalid),
        }
    }

    pub fn view(&self, settings: &Settings) -> Value {
        let value = read_value(settings, self.key);
        let value = if self.secret {
            json!({"redacted": true, "configured": value.as_str().is_some_and(|value| !value.is_empty())})
        } else {
            value
        };
        json!({
            "key": self.key, "value": value, "type": self.value_type(),
            "accepted": self.accepted(), "secret": self.secret,
            "apply": if self.host { "daemon_reload" } else { "desktop_restart" },
        })
    }
}

fn normalize(input: &str) -> String {
    input
        .chars()
        .filter(|ch| !matches!(ch, '-' | '_'))
        .flat_map(char::to_lowercase)
        .collect()
}

fn pointer(key: &str) -> String {
    format!("/{}", key.replace('.', "/"))
}

fn read_value(settings: &Settings, key: &str) -> Value {
    serde_json::to_value(settings)
        .expect("settings serialize")
        .pointer(&pointer(key))
        .expect("catalog key exists")
        .clone()
}

/// Re-read while holding the same lock as the daemon and desktop. Reject bad
/// input before saving; callers never need to hold an outdated settings copy.
pub fn edit(paths: &DataPaths, key: &str, input: Option<&str>) -> Result<Settings, EditError> {
    let setting = find(key)?;
    let value = match input {
        Some(input) => setting.parse(input)?,
        None => read_value(&Settings::default(), key),
    };
    let _lock = DataDirLock::acquire_blocking(&paths.root.join(".settings.lock"))?;
    let current = Settings::load_strict(paths)?;
    let mut document = serde_json::to_value(&current).map_err(CoreError::from)?;
    *document
        .pointer_mut(&pointer(key))
        .expect("catalog key exists") = value;
    // Never format deserialization errors: they can echo a rejected credential.
    let mut next: Settings =
        serde_json::from_value(document).map_err(|_| setting.invalid(setting.accepted()))?;
    next.authentication.validate().map_err(|_| {
        setting
            .invalid("a configured password for password mode, or a valid OTP secret for OTP mode")
    })?;
    next.loaded_from = current.loaded_from;
    next.save(paths)?;
    Ok(next)
}

/// Fields captured when the hosting runner starts. Desktop-only edits must not
/// interrupt an active remote session.
pub fn host_changed(previous: &Settings, next: &Settings) -> bool {
    SETTINGS
        .iter()
        // Power policy is applied live by the daemon without restarting listeners.
        .filter(|setting| setting.host && setting.key != "prevent_idle_sleep")
        .any(|setting| read_value(previous, setting.key) != read_value(next, setting.key))
}

#[cfg(test)]
mod tests;
