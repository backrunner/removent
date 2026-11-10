//! Reusable headless native controller, using the desktop's saved trust and credentials.
use crate::connection::ConnectionProtocol;
use crate::saved::{SavedConnection, SavedConnections};
use crate::{ClientConfig, ClientSession};
use anyhow::{Context, ensure};
use removent_core::{DataPaths, DeviceIdentity, Settings};
use removent_net::{PinState, make_client_endpoint};
use removent_proto::{AuthenticationMode, Caps};
use std::time::Duration;

pub mod broker;
pub struct Connection {
    session: Option<ClientSession>,
    control_revision: std::sync::atomic::AtomicU64,
    attached: Option<broker::Attachment>,
    _publisher: Option<broker::Publisher>,
    // Own the tunnel for the full session, including reconnect-free operation calls.
    _relay: Option<removent_relay::client::ClientBridge>,
}
impl Connection {
    pub fn features(&self) -> u64 {
        self.session.as_ref().map_or_else(
            || self.attached.as_ref().unwrap().info.features,
            |s| s.host_features,
        )
    }
    pub fn shared(&self) -> bool {
        self.attached.is_some()
    }
    pub fn closed(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|s| s.conn.inner().close_reason().is_some())
    }
    pub async fn alive(&self) -> bool {
        if let Some(attached) = &self.attached {
            attached.alive().await
        } else {
            !self.closed()
        }
    }
    pub fn disconnect(&self) {
        if let Some(s) = &self.session {
            s.conn.inner().close(0u32.into(), b"MCP disconnected");
        }
    }
    pub async fn call(
        &self,
        request: &removent_proto::automation::Request,
    ) -> anyhow::Result<removent_proto::automation::Response> {
        use removent_proto::automation::{Command, Failure, Response};
        use std::sync::atomic::Ordering;
        let mut request = request.clone();
        let handover = self.features() & removent_proto::feature_bits::AUTOMATION_HANDOVER != 0;
        if handover && request.command.mutates() && request.control_revision.is_none() {
            request.control_revision = Some(self.control_revision.load(Ordering::SeqCst));
        }
        if !handover
            && (request.control_revision.is_some()
                || matches!(
                    request.command,
                    Command::ControlStatus {} | Command::ResumeControl { .. }
                ))
        {
            return Ok(Response {
                version: 1,
                operation_id: request.operation_id,
                result: Err(Failure::new(
                    "unsupported",
                    "Remote host does not support control handover",
                )),
            });
        }
        let response = tokio::time::timeout(Duration::from_secs(30), async {
            if let Some(s) = &self.session {
                Ok(removent_net::automation::call(&s.conn, &request).await?)
            } else {
                self.attached.as_ref().unwrap().call(&request).await
            }
        })
        .await??;
        if matches!(request.command, Command::ResumeControl { .. })
            && let Ok(value) = &response.result
            && let Some(revision) = value["revision"].as_u64()
        {
            self.control_revision.store(revision, Ordering::SeqCst);
        }
        Ok(response)
    }

    fn attached(attachment: broker::Attachment) -> Self {
        Self {
            session: None,
            control_revision: Default::default(),
            attached: Some(attachment),
            _publisher: None,
            _relay: None,
        }
    }
    pub async fn connect_saved(
        paths: &DataPaths,
        identity: &DeviceIdentity,
        settings: &Settings,
        saved_id: &str,
    ) -> anyhow::Result<Self> {
        let saved = SavedConnections::load(paths)?
            .all()
            .iter()
            .find(|s| s.id == saved_id)
            .cloned()
            .context("Saved connection not found")?;
        Self::connect(paths, identity, settings, &saved).await
    }
    pub async fn connect(
        paths: &DataPaths,
        identity: &DeviceIdentity,
        settings: &Settings,
        saved: &SavedConnection,
    ) -> anyhow::Result<Self> {
        ensure!(
            saved.protocol == ConnectionProtocol::Removent,
            "Computer Use requires a native Removent connection"
        );
        ensure!(
            !saved.credentials_review_required,
            "Review this connection in Removent before connecting through MCP"
        );
        let mut bridge = None;
        let (address, pin_key, supplied) = if let Some(route) = &saved.relay {
            let route = route
                .validated(false)
                .map_err(|_| anyhow::anyhow!("Invalid saved relay route"))?;
            ensure!(
                !route.accept_invalid_certificate,
                "MCP relay connections require certificate verification"
            );
            let host_key = format!("relay:{}:{}", route.endpoint, saved.host);
            let host_trust = crate::host_pins::for_connection(
                paths,
                &host_key,
                route.optional_host_pin(),
                false,
            )?;
            ensure!(
                !host_trust.needs_confirmation,
                "Confirm the host certificate in Removent first"
            );
            if let Some(attached) =
                broker::Attachment::find(paths, host_trust.expected.context("Host pin missing")?)
                    .await?
            {
                return Ok(Self::attached(attached));
            }
            let account = saved
                .credential_account()
                .context("Save the relay credential in Removent first")?
                .to_owned();
            let token = tokio::task::spawn_blocking(move || crate::keychain::load(&account))
                .await??
                .context("Relay credential unavailable")?;
            let mut config = removent_relay::config::TunnelConfig {
                server: route.endpoint.clone(),
                transport: route.transport,
                insecure_loopback: false,
                server_fingerprint: route.server_fingerprint.clone(),
                server_name: route.server_name.clone(),
                accept_invalid_certificate: false,
                room: saved.host.clone(),
                token,
                host_fingerprint: route.host_fingerprint.clone(),
            };
            if !config.is_websocket() {
                let key = format!(
                    "relay-server:{}:{}",
                    config.server,
                    config.tls_server_name()?
                );
                let supplied = if config.server_fingerprint.is_empty() {
                    None
                } else {
                    Some(removent_relay::config::decode_secret(
                        &config.server_fingerprint,
                    )?)
                };
                let trust = crate::host_pins::for_connection(paths, &key, supplied, false)?;
                ensure!(
                    !trust.needs_confirmation,
                    "Confirm the relay certificate in Removent first"
                );
                config.server_fingerprint =
                    hex::encode(trust.expected.context("Relay pin missing")?);
            }
            let value = removent_relay::client::ClientBridge::start_with_confirmation(
                &config, identity, None,
            )
            .await?;
            let address = value.address;
            bridge = Some(value);
            (
                address,
                format!("relay:{}:{}", route.endpoint, saved.host),
                route.optional_host_pin(),
            )
        } else {
            let address = tokio::time::timeout(
                Duration::from_secs(10),
                tokio::net::lookup_host((saved.host.as_str(), saved.port)),
            )
            .await??
            .next()
            .context("No address found")?;
            (address, address.to_string(), None)
        };
        let trust = crate::host_pins::for_connection(paths, &pin_key, supplied, false)?;
        ensure!(
            !trust.needs_confirmation,
            "Confirm the host certificate by connecting in Removent first"
        );
        let host = trust.expected.context("Host certificate pin missing")?;
        if let Some(attached) = broker::Attachment::find(paths, host).await? {
            return Ok(Self::attached(attached));
        }
        let bind = if address.is_ipv6() {
            "[::]:0"
        } else {
            "0.0.0.0:0"
        }
        .parse()?;
        let (endpoint, _) = make_client_endpoint(bind, identity, PinState::new([host], false))?;
        let local = identity.fingerprint_hex();
        let credentials: crate::PinRequest = Box::new(move |mode, sender| {
            tokio::spawn(async move {
                let Some(account) = credential_account(&local, host, mode) else {
                    return;
                };
                if let Ok(Ok(Some(value))) =
                    tokio::task::spawn_blocking(move || crate::keychain::load(&account)).await
                    && mode.valid_input(&value)
                {
                    let _ = sender.send(value);
                }
            });
        });
        let cfg = ClientConfig {
            native_video: false,
            video_quality: saved.video_quality,
            host_identity_verified: true,
            pairing_code: None,
            device_name: settings.device_name.clone(),
            caps: Caps {
                video: true,
                input: true,
                audio: false,
                clipboard: false,
                file: false,
            },
            local_clip: None,
        };
        let session = tokio::time::timeout(
            Duration::from_secs(45),
            crate::connect_session(
                endpoint,
                address,
                identity,
                cfg,
                None,
                None,
                Some(credentials),
            ),
        )
        .await??;
        let publisher =
            broker::Publisher::start(paths, session.conn.clone(), session.host_features).ok();
        Ok(Self {
            session: Some(session),
            control_revision: Default::default(),
            attached: None,
            _publisher: publisher,
            _relay: bridge,
        })
    }
}

/// Shared Keychain namespace. It is bound to identities, never an IP or relay token.
pub fn credential_account(local: &str, host: [u8; 32], mode: AuthenticationMode) -> Option<String> {
    let method = match mode {
        AuthenticationMode::SystemAccount => "system",
        AuthenticationMode::Password => "password",
        _ => return None,
    };
    Some(format!(
        "native-auth:v1:{local}:{}:{method}",
        hex::encode(host)
    ))
}
