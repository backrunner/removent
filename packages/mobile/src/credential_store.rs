//! Handle-independent Keychain editing, called off Swift's MainActor.
use crate::{engine::credentials::account, request::Request};
use anyhow::{Result, ensure};
use removent_client::{
    connection::{ConnectionProtocol, ConnectionRequest},
    host_pins, keychain,
};
use removent_core::{AuthenticationMode, DataPaths};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Load {
        request: Request,
    },
    Save {
        request: Request,
        original_request: Box<Request>,
        edits: Vec<Credential>,
    },
}

// Intentionally no Debug: these values are ephemeral secrets, never bookmarks.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Credential {
    host: [u8; 32],
    mode: AuthenticationMode,
    value: String,
    remember: bool,
}

fn trusted_host(paths: &DataPaths, request: &ConnectionRequest) -> Result<Option<[u8; 32]>> {
    if request.protocol != ConnectionProtocol::Removent || request.pairing_code.is_some() {
        return Ok(None);
    }
    let (destination, supplied) = if let Some(route) = &request.relay {
        (
            format!("relay:{}:{}", route.endpoint, request.address.host),
            route.optional_host_pin(),
        )
    } else {
        (request.address.to_string(), None)
    };
    let trust = host_pins::for_connection(paths, &destination, supplied, false)?;
    Ok(trust.expected.filter(|_| !trust.needs_confirmation))
}

pub fn dispatch(paths: &DataPaths, command: Command) -> Result<Value> {
    // The engine created this identity before exposing the storage entry point.
    ensure!(
        paths.device_key().exists(),
        "Controller identity unavailable"
    );
    let local = removent_core::identity::load_or_create(paths, "Removent")?.fingerprint_hex();
    match command {
        Command::Load { request } => {
            let mut credentials = Vec::new();
            if let Some(host) = trusted_host(paths, &request.connection()?)? {
                for mode in [
                    AuthenticationMode::SystemAccount,
                    AuthenticationMode::Password,
                ] {
                    let key = account(&local, host, mode).unwrap();
                    if let Some(value) = keychain::load(&key)? {
                        credentials.push(Credential {
                            host,
                            mode,
                            value,
                            remember: true,
                        });
                    }
                }
            }
            Ok(serde_json::to_value(credentials)?)
        }
        Command::Save {
            request,
            original_request,
            edits,
        } => {
            ensure!(edits.len() <= 2, "Too many credential edits");
            let request = request.connection()?;
            let original = original_request.connection()?;
            ensure!(
                request.protocol == original.protocol
                    && request.address == original.address
                    && request.relay == original.relay,
                "Credential destination changed; reopen the connection editor"
            );
            let host = trusted_host(paths, &request)?;
            for edit in &edits {
                ensure!(
                    Some(edit.host) == host && account(&local, edit.host, edit.mode).is_some(),
                    "Credential identity changed; reopen the connection editor"
                );
                ensure!(
                    !edit.remember || edit.mode.valid_input(&edit.value),
                    "Check the username and password and try again"
                );
            }
            for edit in edits {
                let key = account(&local, edit.host, edit.mode).unwrap();
                if edit.remember {
                    keychain::store(&key, &edit.value)?;
                } else {
                    keychain::delete(&key)?;
                }
            }
            Ok(json!(null))
        }
    }
}

#[cfg(test)]
mod tests;
