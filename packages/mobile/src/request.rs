use anyhow::{Result, bail, ensure};
use removent_client::connection::{
    ConnectionAddress, ConnectionProtocol, ConnectionRequest, RelayRoute,
};
use removent_proto::{ControlMsg, KeyKind, KeyModifiers, MouseKind, ScrollPhase};
use serde::Deserialize;

/// Secrets exist only in this transient request or the platform Keychain.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub protocol: ConnectionProtocol,
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub accept_invalid_certificate: bool,
    #[serde(default)]
    pub relay: Option<RelayRoute>,
    #[serde(default = "enabled")]
    pub audio: bool,
    #[serde(default = "enabled")]
    pub clipboard: bool,
}

fn enabled() -> bool {
    true
}

impl Request {
    pub fn connection(&self) -> Result<ConnectionRequest> {
        ensure!(
            self.host.len() <= 1024
                && self.username.len() <= 256
                && self.password.len() <= 4096
                && self.domain.len() <= 256,
            "Connection field is too long"
        );
        let pairing_code = (self.protocol == ConnectionProtocol::Removent)
            .then(|| removent_core::pairing_invitation::PairingCode::parse(&self.host))
            .flatten();
        let relay = if let Some(route) = &self.relay {
            ensure!(
                self.protocol == ConnectionProtocol::Removent,
                "Relay requires Removent"
            );
            Some(
                RelayRoute::parse_for_connection(
                    &route.endpoint,
                    route.transport,
                    &route.server_fingerprint,
                    if pairing_code.is_some() {
                        ""
                    } else {
                        &route.host_fingerprint
                    },
                    pairing_code.is_some(),
                )
                .and_then(|parsed| {
                    parsed.with_tls(&route.server_name, route.accept_invalid_certificate)
                })
                .map_err(anyhow::Error::msg)?,
            )
        } else {
            None
        };
        let address = if let Some(code) = &pairing_code {
            ConnectionAddress {
                host: code.room(),
                port: if relay.is_some() {
                    0
                } else {
                    self.protocol.default_port()
                },
            }
        } else if relay.is_some() {
            ConnectionAddress::relay_room(&self.host)?
        } else if self.protocol == ConnectionProtocol::Removent {
            ConnectionAddress::parse_native(&self.host, &self.port.to_string())?
        } else {
            ConnectionAddress::parse(&self.host, &self.port.to_string())?
        };
        if self.protocol == ConnectionProtocol::Rdp {
            ensure!(!self.username.trim().is_empty(), "RDP requires a username");
        }
        Ok(ConnectionRequest {
            pairing_code,
            protocol: self.protocol,
            address,
            username: self.username.clone(),
            password: self.password.clone(),
            domain: self.domain.clone(),
            accept_invalid_certificate: self.accept_invalid_certificate,
            relay,
        })
    }
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Input {
    Pointer {
        width: u32,
        height: u32,
        x: f32,
        y: f32,
        buttons: u8,
        action: MouseKind,
    },
    Scroll {
        dx: f32,
        dy: f32,
        phase: ScrollPhase,
    },
    Key {
        code: u16,
        modifiers: u8,
        down: bool,
        unicode: Option<char>,
    },
    Text {
        text: String,
    },
}

impl Input {
    pub fn messages(self) -> Result<Vec<ControlMsg>> {
        Ok(match self {
            Self::Pointer {
                width,
                height,
                x,
                y,
                buttons,
                action,
            } => {
                ensure!(
                    width > 0
                        && height > 0
                        && u64::from(width) * u64::from(height) <= 16 * 1024 * 1024,
                    "Invalid frame geometry"
                );
                ensure!(
                    x.is_finite() && y.is_finite() && buttons <= 7,
                    "Invalid pointer"
                );
                vec![
                    ControlMsg::FrameGeometry { width, height },
                    ControlMsg::MouseEvent {
                        display_id: 0,
                        x_px: x.clamp(0., (width - 1) as f32),
                        y_px: y.clamp(0., (height - 1) as f32),
                        buttons,
                        kind: action,
                    },
                ]
            }
            Self::Scroll { dx, dy, phase } => {
                ensure!(dx.is_finite() && dy.is_finite(), "Invalid scroll");
                vec![ControlMsg::ScrollEvent {
                    display_id: 0,
                    dx_mm: dx.clamp(-100., 100.),
                    dy_mm: dy.clamp(-100., 100.),
                    phase,
                }]
            }
            Self::Key {
                code,
                modifiers,
                down,
                unicode,
            } => vec![ControlMsg::KeyEvent {
                vk_code: code,
                modifiers: KeyModifiers::from_bits_truncate(modifiers),
                kind: if down { KeyKind::Down } else { KeyKind::Up },
                unicode,
            }],
            Self::Text { text } => {
                if text.chars().count() > 64 {
                    bail!("Send text in batches of up to 64 characters");
                }
                text.chars()
                    .flat_map(|ch| {
                        [KeyKind::Down, KeyKind::Up].map(|kind| ControlMsg::KeyEvent {
                            vk_code: u16::MAX,
                            modifiers: KeyModifiers::empty(),
                            kind,
                            unicode: Some(ch),
                        })
                    })
                    .collect()
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_routes_and_preserves_certificate_verification() {
        let request: Request = serde_json::from_value(serde_json::json!({
            "protocol":"rdp", "host":"pc.local", "port":3389, "username":"user"
        }))
        .unwrap();
        assert!(!request.connection().unwrap().accept_invalid_certificate);
        let request: Request = serde_json::from_value(serde_json::json!({
            "protocol":"vnc", "host":"vnc://host", "port":5900
        }))
        .unwrap();
        assert!(request.connection().is_err());
    }
    #[test]
    fn invitation_uses_public_route_and_keeps_secret_transient() {
        let request: Request = serde_json::from_value(
            serde_json::json!({ "protocol":"removent", "host":"123456-654321", "port":0 }),
        )
        .unwrap();
        let connection = request.connection().unwrap();
        assert_eq!(connection.address.host, "pair-123456");
        assert_eq!(connection.pairing_code.unwrap().secret(), "654321");
        let request: Request = serde_json::from_value(serde_json::json!({ "protocol":"removent", "host":"123456654321", "port":0, "relay": {"endpoint":"removent://relay.example:443", "transport":"websocket", "server_fingerprint":"", "host_fingerprint":""} })).unwrap();
        assert!(request.connection().is_ok());
    }
    #[test]
    fn pointer_geometry_precedes_clamped_input() {
        let msgs = Input::Pointer {
            width: 100,
            height: 50,
            x: 1000.,
            y: -4.,
            buttons: 1,
            action: MouseKind::LeftDown,
        }
        .messages()
        .unwrap();
        assert!(matches!(
            msgs[0],
            ControlMsg::FrameGeometry {
                width: 100,
                height: 50
            }
        ));
        assert!(matches!(
            msgs[1],
            ControlMsg::MouseEvent {
                x_px: 99.,
                y_px: 0.,
                ..
            }
        ));
        assert!(
            Input::Scroll {
                dx: f32::NAN,
                dy: 0.,
                phase: ScrollPhase::Changed
            }
            .messages()
            .is_err()
        );
    }
}
