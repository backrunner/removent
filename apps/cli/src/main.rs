//! removent-cli: M1 smoke-test tool (milestones.md M1).
//!
//! Usage:
//! ```text
//! removent-cli ping <addr|name|short-fingerprint> [--pin code] [--count n] [--timeout secs] [--name device-name]
//! ```
//!
//! Flow: discovery (direct address or mDNS browse) → pairing (SPAKE2, when the
//! peer is unknown) → control-stream negotiation → Ping/Pong echo → print RTT stats.

use anyhow::{Context, bail};
use futures::{SinkExt, StreamExt};
use removent_core::{DataPaths, DeviceIdentity, identity};
use removent_net::{
    ControlItem, DiscoveryBrowser, PairingMsg, PinState, RvpConnection, client_begin,
    client_confirm_check, client_verify, make_client_endpoint,
};
use removent_proto::{
    Caps, ControlMsg, EndReason, HandshakeClient, Hello, MAGIC, NegotiateAck, PROTO_VERSION,
};
use rust_i18n::t;

use std::net::SocketAddr;
use std::time::{Duration, Instant};

rust_i18n::i18n!("locales");

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let paths = DataPaths::resolve();
    if std::env::args().nth(1).as_deref() == Some("cloud-sync") {
        // Bounded stdin/stdout protocol for the user-owned CloudKit helper. Never
        // initialize host identity, touch Keychain, or log connection payloads.
        use std::io::Read;
        let mut input = String::new();
        std::io::stdin()
            .take(8 * 1024 * 1024 + 1)
            .read_to_string(&mut input)?;
        anyhow::ensure!(input.len() <= 8 * 1024 * 1024, "Sync command too large");
        let result = serde_json::from_str(&input)
            .map_err(removent_core::CoreError::from)
            .and_then(|command| removent_client::cloud_sync::dispatch(&paths, command));
        let response = match result {
            Ok(value) => serde_json::json!({"ok":true,"value":value}),
            Err(error) => serde_json::json!({"ok":false,"error":error.to_string()}),
        };
        println!("{}", response);
        return Ok(());
    }
    removent_core::logging::init_logging(&paths);
    removent_core::logging::install_panic_hook(&paths);
    let settings = removent_core::Settings::load(&paths).unwrap_or_default();
    rust_i18n::set_locale(removent_core::resolve_locale(settings.language));

    let raw: Vec<String> = std::env::args().skip(1).collect();
    match raw.first().map(String::as_str) {
        Some("identity") => {
            let identity = identity::load_or_create(&paths, &settings.device_name)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "certificate_fingerprint": identity.fingerprint_hex(),
                    "public_key": hex::encode(identity.verifying_key().as_bytes()),
                }))?
            );
            Ok(())
        }
        Some("ping") => cmd_ping(parse_args(&raw[1..])?).await,
        Some("daemon") => {
            let action = raw.get(1).map(String::as_str).unwrap_or("status");
            cmd_daemon(action).await
        }
        Some("--help") | Some("-h") | None => {
            print!("{}", t!("usage"));
            Ok(())
        }
        Some(other) => {
            eprintln!("{}\n", t!("error.unknown_subcommand", cmd = other));
            print!("{}", t!("usage"));
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_profile_and_native_address_are_exclusive() {
        let parse =
            |values: &[&str]| parse_args(&values.iter().map(|v| v.to_string()).collect::<Vec<_>>());
        let args = parse(&["--relay-profile", "/private/office.toml", "--count", "20"]).unwrap();
        assert_eq!(
            args.relay_profile.unwrap(),
            std::path::PathBuf::from("/private/office.toml")
        );
        assert_eq!(args.count, 20);
        assert!(parse(&["removent://localhost:48688"]).is_ok());
        for bad in [
            vec![],
            vec!["relay://office"],
            vec!["quic://localhost:48688"],
            vec!["wss://localhost:443"],
            vec!["--relay-profile"],
            vec!["localhost", "--relay-profile", "office.toml"],
        ] {
            assert!(parse(&bad).is_err());
        }
    }

    #[tokio::test]
    async fn native_uri_resolves_loopback_without_discovery() {
        for target in ["removent://127.0.0.1:48688", "removent://[::1]:48688"] {
            let address = resolve_target(target, Duration::from_secs(1))
                .await
                .unwrap();
            assert!(address.ip().is_loopback());
            assert_eq!(address.port(), 48688);
        }
    }
}

mod daemon;
mod ping;
use daemon::cmd_daemon;
#[cfg(test)]
use ping::resolve_target;
use ping::{cmd_ping, parse_args};
