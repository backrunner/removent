//! Versioned, bounded automation RPC on an authenticated RVP session.
//! These JSON messages do not change existing postcard enum layouts.
use serde::{Deserialize, Serialize};

pub const VERSION: u16 = 1;
pub const MAGIC: &[u8; 4] = b"AUT1";
pub const MAX_REQUEST_BYTES: usize = 128 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 24 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub version: u16,
    pub operation_id: String,
    /// Revision captured before dispatch; prevents late mutations crossing a human takeover.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_revision: Option<u64>,
    pub command: Command,
}

#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Capabilities {},
    ControlStatus {},
    ResumeControl {
        revision: u64,
    },
    State {},
    OpenApp {
        bundle_id: String,
    },
    Snapshot {
        target: String,
    },
    Screenshot {
        display_id: u64,
        max_width: u32,
    },
    Click {
        point: Point,
        button: Button,
        count: u8,
        modifiers: Vec<String>,
        hold_ms: u32,
    },
    Move {
        point: Point,
    },
    Drag {
        from: Point,
        to: Point,
        duration_ms: u32,
    },
    Scroll {
        point: Point,
        dx: f64,
        dy: f64,
    },
    PressKey {
        key: String,
        hold_ms: u32,
    },
    TypeText {
        text: String,
    },
    ElementAction {
        reference: String,
        name: String,
    },
    Focus {
        reference: String,
    },
    SetValue {
        reference: String,
        value: String,
    },
    SelectText {
        reference: String,
        text: String,
        prefix: Option<String>,
        suffix: Option<String>,
        placement: TextPlacement,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Point {
    /// Reference to a host-produced screenshot, never an arbitrary display geometry.
    pub observation: u64,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TextPlacement {
    Select,
    Before,
    After,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub version: u16,
    pub operation_id: String,
    pub result: Result<serde_json::Value, Failure>,
}

#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[error("{code}: {message}")]
pub struct Failure {
    pub code: String,
    pub message: String,
}

impl Failure {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl Command {
    pub fn mutates(&self) -> bool {
        !matches!(
            self,
            Self::Capabilities {}
                | Self::ControlStatus {}
                | Self::State {}
                | Self::Snapshot { .. }
                | Self::Screenshot { .. }
        )
    }

    pub fn validate(&self) -> Result<(), Failure> {
        let valid_point = |p: &Point| {
            p.observation > 0 && p.x.is_finite() && p.y.is_finite() && p.x >= 0.0 && p.y >= 0.0
        };
        let short = |s: &str| !s.is_empty() && s.len() <= 512 && !s.chars().any(char::is_control);
        let valid = match self {
            Self::Capabilities {} | Self::State {} | Self::ControlStatus {} => true,
            Self::ResumeControl { revision } => *revision < (1u64 << 53),
            Self::OpenApp { bundle_id } => {
                short(bundle_id)
                    && bundle_id.contains('.')
                    && bundle_id
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b".-_".contains(&c))
            }
            Self::Snapshot { target } => short(target),
            Self::Screenshot { max_width, .. } => (320..=3840).contains(max_width),
            Self::Click {
                point,
                count,
                modifiers,
                hold_ms,
                ..
            } => {
                valid_point(point)
                    && (1..=3).contains(count)
                    && *hold_ms <= 3000
                    && modifiers.len() <= 4
                    && modifiers
                        .iter()
                        .all(|m| matches!(m.as_str(), "shift" | "control" | "alt" | "meta"))
            }
            Self::Move { point } => valid_point(point),
            Self::Drag {
                from,
                to,
                duration_ms,
            } => {
                valid_point(from)
                    && valid_point(to)
                    && from.observation == to.observation
                    && (50..=5000).contains(duration_ms)
            }
            Self::Scroll { point, dx, dy } => {
                valid_point(point)
                    && dx.is_finite()
                    && dy.is_finite()
                    && dx.abs() <= 10000.0
                    && dy.abs() <= 10000.0
            }
            Self::PressKey { key, hold_ms } => short(key) && *hold_ms <= 3000,
            Self::TypeText { text } => !text.is_empty() && text.len() <= 16 * 1024,
            Self::ElementAction { reference, name } => short(reference) && short(name),
            Self::Focus { reference } => short(reference),
            Self::SetValue { reference, value } => short(reference) && value.len() <= 16 * 1024,
            Self::SelectText {
                reference,
                text,
                prefix,
                suffix,
                ..
            } => {
                short(reference)
                    && !text.is_empty()
                    && text.len() <= 16 * 1024
                    && prefix.as_ref().is_none_or(|s| s.len() <= 1024)
                    && suffix.as_ref().is_none_or(|s| s.len() <= 1024)
            }
        };
        if valid {
            Ok(())
        } else {
            Err(Failure::new(
                "invalid_argument",
                "Invalid or oversized automation arguments",
            ))
        }
    }
}

impl Request {
    pub fn validate(&self) -> Result<(), Failure> {
        if self.version != VERSION {
            return Err(Failure::new(
                "unsupported_version",
                "Automation protocol version mismatch",
            ));
        }
        if self.operation_id.is_empty()
            || self.operation_id.len() > 64
            || !self
                .operation_id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
        {
            return Err(Failure::new(
                "invalid_argument",
                "operation_id must contain 1 to 64 ASCII letters, digits, underscores or hyphens",
            ));
        }
        if self
            .control_revision
            .is_some_and(|revision| revision >= (1u64 << 53))
        {
            return Err(Failure::new(
                "invalid_argument",
                "control_revision must be a JavaScript-safe nonnegative integer",
            ));
        }
        self.command.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn handover_revision_is_optional_on_old_wire_and_bounded_when_present() {
        let legacy = r#"{"version":1,"operation_id":"first","command":{"action":"state"}}"#;
        let mut request: Request = serde_json::from_str(legacy).unwrap();
        request.validate().unwrap();
        assert_eq!(request.control_revision, None);
        assert!(
            !serde_json::to_value(&request)
                .unwrap()
                .as_object()
                .unwrap()
                .contains_key("control_revision")
        );
        request.control_revision = Some(1u64 << 53);
        assert_eq!(request.validate().unwrap_err().code, "invalid_argument");
        assert!(
            Command::ResumeControl {
                revision: 1u64 << 53
            }
            .validate()
            .is_err()
        );
    }
    #[test]
    fn rejects_unbounded_and_nonfinite_input_and_unknown_fields() {
        assert!(
            Command::Move {
                point: Point {
                    observation: 1,
                    x: f64::NAN,
                    y: 0.
                }
            }
            .validate()
            .is_err()
        );
        assert!(
            Command::TypeText {
                text: "x".repeat(16385)
            }
            .validate()
            .is_err()
        );
        assert!(serde_json::from_str::<Command>(r#"{"action":"state","extra":true}"#).is_err());
        assert!(
            Command::OpenApp {
                bundle_id: "-a /Applications/Test.app".into()
            }
            .validate()
            .is_err()
        );
    }
}
