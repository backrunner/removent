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

/// daemon management subcommand: send a request over UDS and print the response JSON.
async fn cmd_daemon(action: &str) -> anyhow::Result<()> {
    use removent_core::ipc::{IpcRequest, read_msg, write_msg};

    let paths = DataPaths::resolve();
    #[cfg(target_os = "macos")]
    if matches!(
        action,
        "start" | "stop" | "restart" | "login-on" | "login-off" | "service-status"
    ) {
        let exe = std::env::current_exe()?.with_file_name("removentd");
        let service = removent_core::service::Service::new(paths, exe)?;
        let action = action.to_owned();
        // launchctl and the startup readiness wait are blocking operations.
        let status = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            match action.as_str() {
                "start" => service.start()?,
                "stop" => service.stop()?,
                "restart" => service.restart()?,
                "login-on" => service.set_launch_at_login(true)?,
                "login-off" => service.set_launch_at_login(false)?,
                _ => {}
            }
            service.status()
        })
        .await??;
        println!("{}", serde_json::to_string_pretty(&status)?);
        return Ok(());
    }

    let req = match action {
        "status" => IpcRequest::Status,
        "permissions" => IpcRequest::RequestPermissions,
        "enable" => IpcRequest::SetEnabled { on: true },
        "disable" => IpcRequest::SetEnabled { on: false },
        other => bail!(t!("daemon.unknown_action", action = other)),
    };

    let (mut r, mut w) = removent_core::ipc::connect(&paths)
        .await
        .context(t!("daemon.connect_failed"))?;
    write_msg(&mut w, &req)
        .await
        .context(t!("daemon.send_failed"))?;

    // The event stream may interleave with responses; take the first response (with a timeout).
    let resp: serde_json::Value = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let v: serde_json::Value = read_msg(&mut r)
                .await
                .context(t!("daemon.read_failed"))?
                .context(t!("daemon.disconnected"))?;
            match v.get("type").and_then(serde_json::Value::as_str) {
                Some("status" | "ok" | "error") => return Ok::<_, anyhow::Error>(v),
                _ => continue,
            }
        }
    })
    .await
    .context(t!("daemon.wait_timeout"))??;
    if resp.get("type").and_then(serde_json::Value::as_str) == Some("error") {
        bail!(
            "{}",
            resp.get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Daemon request failed")
        );
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&resp).context(t!("daemon.serialize_failed"))?
    );
    Ok(())
}

struct PingArgs {
    target: String,
    pin: Option<String>,
    count: u32,
    timeout: Duration,
    name: String,
}

fn parse_args(raw: &[String]) -> anyhow::Result<PingArgs> {
    let mut target = None;
    let mut pin = None;
    let mut count = 4u32;
    let mut timeout = 10u64;
    let mut name = "RemoventCLI".to_string();

    let mut it = raw.iter();
    while let Some(arg) = it.next() {
        let inline = |prefix: &str| arg.strip_prefix(prefix).filter(|v| !v.is_empty());
        let mut next_val = || {
            it.next()
                .context(t!("args.missing_value", arg = arg))
                .map(String::as_str)
        };
        if let Some(v) = inline("--pin=") {
            pin = Some(v.to_string());
        } else if arg == "--pin" {
            pin = Some(next_val()?.to_string());
        } else if let Some(v) = inline("--count=") {
            count = v.parse().context(t!("args.count_invalid"))?;
        } else if arg == "--count" {
            count = next_val()?.parse().context(t!("args.count_invalid"))?;
        } else if let Some(v) = inline("--timeout=") {
            timeout = v.parse().context(t!("args.timeout_invalid"))?;
        } else if arg == "--timeout" {
            timeout = next_val()?.parse().context(t!("args.timeout_invalid"))?;
        } else if let Some(v) = inline("--name=") {
            name = v.to_string();
        } else if arg == "--name" {
            name = next_val()?.to_string();
        } else if !arg.starts_with('-') && target.is_none() {
            target = Some(arg.clone());
        } else {
            bail!(t!("args.unrecognized", arg = arg));
        }
    }
    Ok(PingArgs {
        target: target.context(t!("args.missing_target"))?,
        pin,
        count: count.max(1),
        timeout: Duration::from_secs(timeout.max(1)),
        name,
    })
}

/// Target resolution: a bare address is used directly; otherwise browse mDNS by
/// name/short-fingerprint/instance-name. Multiple simultaneous matches are an
/// ambiguity error instead of silently picking the first one.
async fn resolve_target(target: &str, timeout: Duration) -> anyhow::Result<SocketAddr> {
    if let Ok(addr) = target.parse::<SocketAddr>() {
        return Ok(addr);
    }

    let browser = DiscoveryBrowser::start()
        .map_err(|e| anyhow::anyhow!(t!("resolve.mdns_failed", err = e.to_string())))?;
    let mut rx = browser.subscribe_table();
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let snapshot = rx.borrow_and_update().clone();
        let hits: Vec<_> = snapshot
            .values()
            .filter(|e| {
                e.addr.is_some()
                    && (e.name == target
                        || e.short_fp.starts_with(target)
                        || e.instance.contains(target))
            })
            .collect();
        match hits.len() {
            0 => {}
            1 => {
                let e = hits[0];
                let addr = e.addr.expect("filtered above");
                eprintln!(
                    "{}",
                    t!(
                        "resolve.discovered",
                        name = e.name.as_str(),
                        fp = e.short_fp.as_str(),
                        addr = addr
                    )
                );
                return Ok(addr);
            }
            n => {
                let list = hits
                    .iter()
                    .map(|e| format!("{}({} @ {:?})", e.name, e.short_fp, e.addr))
                    .collect::<Vec<_>>()
                    .join(", ");
                bail!(t!(
                    "resolve.ambiguous",
                    target = target,
                    count = n,
                    list = list
                ));
            }
        }
        if tokio::time::Instant::now() >= deadline {
            let seen: Vec<&str> = snapshot.values().map(|e| e.name.as_str()).collect();
            if seen.is_empty() {
                bail!(t!("resolve.timeout_none"));
            }
            bail!(t!(
                "resolve.timeout_unmatched",
                target = target,
                seen = seen.join(", ")
            ));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn now_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

/// Extract the long-term public key from the peer TLS certificate (self-signed
/// ed25519) for pairing signature verification.
fn peer_verifying_key(conn: &RvpConnection) -> Option<ed25519_dalek::VerifyingKey> {
    let chain = conn
        .inner()
        .peer_identity()?
        .downcast::<Vec<removent_net::quinn::rustls::pki_types::CertificateDer<'static>>>()
        .ok()?;
    let der = chain.first()?;
    // Fixed Ed25519 SubjectPublicKeyInfo prefix, followed by the 32-byte public key.
    const SPKI_PREFIX: [u8; 12] = [
        0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
    ];
    let bytes: &[u8] = der.as_ref();
    let pos = bytes
        .windows(SPKI_PREFIX.len())
        .position(|w| w == SPKI_PREFIX)?;
    let key: [u8; 32] = bytes
        .get(pos + SPKI_PREFIX.len()..pos + SPKI_PREFIX.len() + 32)?
        .try_into()
        .ok()?;
    ed25519_dalek::VerifyingKey::from_bytes(&key).ok()
}

fn read_pin_stdin() -> String {
    use std::io::Write;
    eprint!("{}", t!("pin.prompt"));
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    line.trim().to_string()
}

/// Concurrent pairing task: for unknown peers the host waits inline on this stream
/// after the handshake (protocol.md §4.3). If the host already trusts us it never
/// accepts the stream, so the task simply times out and exits quietly.
fn spawn_pairing(
    conn: RvpConnection,
    identity: DeviceIdentity,
    fp_peer_full: String,
    pin: Option<String>,
) -> tokio::task::JoinHandle<()> {
    let fp_self = identity.fingerprint_hex();
    tokio::spawn(async move {
        let outcome: anyhow::Result<()> = async {
            let (mut psink, mut psource) = conn
                .open_pairing()
                .await
                .map_err(|e| anyhow::anyhow!(t!("pairing.open_stream", err = e.to_string())))?;

            let (begin_msg, nonce_c) = client_begin(&fp_self);
            psink
                .send(begin_msg)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            let challenge = tokio::time::timeout(Duration::from_secs(15), psource.next())
                .await
                .map_err(|_| anyhow::anyhow!(t!("pairing.challenge_timeout")))?
                .transpose()
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .ok_or_else(|| anyhow::anyhow!(t!("pairing.stream_closed")))?;
            let PairingMsg::Challenge { ref nonce_h, .. } = challenge else {
                bail!(t!("pairing.expected_challenge"));
            };

            let pin = match pin {
                Some(p) => p,
                None => tokio::task::spawn_blocking(read_pin_stdin)
                    .await
                    .unwrap_or_default(),
            };
            if pin.is_empty() {
                bail!(t!("pairing.no_pin"));
            }

            let (verify_msg, shared) = client_verify(
                &nonce_c,
                &challenge,
                &fp_self,
                &fp_peer_full,
                &pin,
                &identity,
            )
            .map_err(|e| anyhow::anyhow!(t!("pairing.spake2_verify", err = e.to_string())))?;
            psink
                .send(verify_msg)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            let confirm = tokio::time::timeout(Duration::from_secs(15), psource.next())
                .await
                .map_err(|_| anyhow::anyhow!(t!("pairing.confirm_timeout")))?
                .transpose()
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .ok_or_else(|| anyhow::anyhow!(t!("pairing.stream_closed")))?;
            let peer_vk = peer_verifying_key(&conn)
                .ok_or_else(|| anyhow::anyhow!(t!("pairing.no_peer_key")))?;
            client_confirm_check(
                &shared,
                &nonce_c,
                nonce_h,
                &fp_self,
                &fp_peer_full,
                &confirm,
                &peer_vk,
            )
            .map_err(|e| anyhow::anyhow!(t!("pairing.confirm_failed", err = e.to_string())))?;
            Ok(())
        }
        .await;
        match outcome {
            Ok(()) => eprintln!("{}", t!("pairing.done")),
            Err(e) => eprintln!("{}", t!("pairing.failed", err = format!("{e}"))),
        }
    })
}

/// Drain media streams in the background so the host is not blocked by flow
/// control (debug-verified quinn pitfall #13).
fn spawn_media_drain(conn: RvpConnection) {
    tokio::spawn(async move {
        while let Ok(mut stream) = conn.accept_media_stream().await {
            tokio::spawn(async move {
                let mut buf = vec![0u8; 64 * 1024];
                while let Ok(Some(_)) = stream.read(&mut buf).await {}
            });
        }
    });
}

async fn expect_control(
    source: &mut removent_net::ControlSource,
    what: &str,
) -> anyhow::Result<Box<ControlMsg>> {
    let item = tokio::time::timeout(Duration::from_secs(15), source.next())
        .await
        .map_err(|_| anyhow::anyhow!(t!("control.wait_timeout", what = what)))?
        .transpose()
        .map_err(|e| anyhow::anyhow!(t!("control.stream_error", err = e.to_string())))?
        .ok_or_else(|| anyhow::anyhow!(t!("control.stream_closed")))?;
    match item {
        ControlItem::Msg(m) => Ok(m),
        ControlItem::Skipped => Err(anyhow::anyhow!(t!("control.unexpected", what = what))),
    }
}

async fn cmd_ping(args: PingArgs) -> anyhow::Result<()> {
    let paths = DataPaths::resolve();
    let id = identity::load_or_create(&paths, &args.name).context(t!("error.load_identity"))?;

    let addr = resolve_target(&args.target, args.timeout).await?;

    let (ep, _pin_state) = make_client_endpoint(
        SocketAddr::from(([0, 0, 0, 0], 0)),
        &id,
        PinState::new([], true),
    )
    .map_err(|e| anyhow::anyhow!(t!("error.create_endpoint", err = e.to_string())))?;

    println!("{}", t!("session.connecting", addr = addr));
    // Brief retries while the server is not ready yet (same as the client engine).
    let conn = {
        let mut attempt = 0u32;
        loop {
            match ep.connect(addr, "removent") {
                Ok(connecting) => match connecting.await {
                    Ok(qconn) => break RvpConnection::new(qconn),
                    Err(e) => {
                        attempt += 1;
                        if attempt > 50 {
                            bail!(t!("error.quic_connect", err = e.to_string()));
                        }
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                },
                Err(e) => bail!(t!("error.quic_connect", err = e.to_string())),
            }
        }
    };

    let peer_fp = conn
        .peer_fingerprint()
        .map(|fp| fp.iter().map(|b| format!("{b:02x}")).collect::<String>())
        .context(t!("error.no_peer_fp"))?;

    let (_ack, mut sink, mut source) = conn
        .connect_handshake(HandshakeClient {
            magic: MAGIC,
            proto_version: PROTO_VERSION,
            // Honest advertisement: file-transfer/two-way-audio are not implemented.
            feature_bits: 0,
            hello: Hello {
                app_version: removent_core::APP_VERSION.to_string(),
                device_name: args.name.clone(),
                os_version: std::env::consts::OS.to_string(),
                caps: Caps::all(),
                resume_token: None,
            },
        })
        .await
        .map_err(|e| anyhow::anyhow!(t!("error.handshake", err = e.to_string())))?;

    // Same as the client engine: start pairing concurrently after the handshake,
    // then send SessionRequest.
    let pairing_task = spawn_pairing(conn.clone(), id.clone(), peer_fp, args.pin.clone());

    sink.send(ControlMsg::SessionRequest { caps: Caps::all() })
        .await
        .map_err(|e| anyhow::anyhow!(t!("error.send_session_request", err = e.to_string())))?;

    match *expect_control(&mut source, &t!("control.what.session_accept")).await? {
        ControlMsg::SessionAccept => {}
        ControlMsg::SessionReject { reason } => {
            bail!(t!("error.session_rejected", reason = format!("{reason:?}")))
        }
        other => bail!(t!(
            "error.expected_session_accept",
            msg = format!("{other:?}")
        )),
    }

    let offer = *expect_control(&mut source, &t!("control.what.negotiate_offer")).await?;
    let ControlMsg::NegotiateOffer { n } = offer else {
        bail!(t!("error.expected_negotiate_offer"));
    };
    sink.send(ControlMsg::NegotiateReply {
        ack: Box::new(NegotiateAck {
            video: n.video,
            audio: n.audio,
            resume_token: None,
        }),
    })
    .await
    .map_err(|e| anyhow::anyhow!(t!("error.send_negotiate_reply", err = e.to_string())))?;
    let _final = *expect_control(&mut source, &t!("control.what.final_negotiate_reply")).await?;

    pairing_task.abort();
    spawn_media_drain(conn.clone());

    println!(
        "{}",
        t!(
            "session.ready",
            codec = format!("{:?}", n.video.codec),
            fps = n.video.max_fps,
            rate = n.audio.sample_rate,
            count = args.count
        )
    );

    let mut rtts = Vec::with_capacity(args.count as usize);
    for i in 0..args.count {
        let ts = now_us();
        let sent = Instant::now();
        sink.send(ControlMsg::Ping { ts_us: ts })
            .await
            .map_err(|e| anyhow::anyhow!(t!("error.send_ping", err = e.to_string())))?;
        loop {
            let msg = *expect_control(&mut source, &t!("control.what.pong")).await?;
            match msg {
                ControlMsg::Pong { ts_us: echo } if echo == ts => break,
                ControlMsg::Pong { .. } | ControlMsg::QualityControl { .. } => {}
                _ => {}
            }
        }
        let rtt = sent.elapsed();
        rtts.push(rtt);
        println!(
            "{}",
            t!(
                "ping.line",
                n = i + 1,
                ms = format!("{:.2}", rtt.as_secs_f64() * 1_000.0)
            )
        );
    }

    rtts.sort();
    let min = rtts.first().copied().unwrap_or_default();
    let max = rtts.last().copied().unwrap_or_default();
    let avg = rtts.iter().sum::<Duration>() / rtts.len().max(1) as u32;
    println!(
        "{}",
        t!(
            "ping.stats",
            samples = rtts.len(),
            min = format!("{:.2}", min.as_secs_f64() * 1_000.0),
            avg = format!("{:.2}", avg.as_secs_f64() * 1_000.0),
            max = format!("{:.2}", max.as_secs_f64() * 1_000.0),
            est = format!("{:.2}", conn.rtt_estimate())
        )
    );

    let _ = sink
        .send(ControlMsg::SessionEnd {
            reason: EndReason::ClientClosed,
        })
        .await;
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let paths = DataPaths::resolve();
    let settings = removent_core::Settings::load(&paths).unwrap_or_default();
    rust_i18n::set_locale(removent_core::resolve_locale(settings.language));

    let raw: Vec<String> = std::env::args().skip(1).collect();
    match raw.first().map(String::as_str) {
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
