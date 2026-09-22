//! Isolated relay benchmark; use scripts/benchmark_relay.py to sample only its PID.
//! Synthetic inner RVP media, pinned device identities, real outer authentication.
use anyhow::{Context, Result, ensure};
use removent_core::{DataPaths, DeviceIdentity, identity, removent_uri::RelayTransport};
use removent_net::{PinState, RvpConnection, make_client_endpoint, make_server_endpoint};
use removent_relay::{
    client::{self, ClientBridge},
    config::TunnelConfig,
};
use serde_json::json;
use std::{
    io::Write,
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
};

fn event(value: serde_json::Value) {
    println!("{value}");
    std::io::stdout().flush().unwrap();
    if value["event"] == "end" {
        // Do not disconnect/kill the measured process before the sampler has
        // consumed its final CPU/RSS counter. The Python driver acknowledges.
        let mut ack = String::new();
        std::io::stdin().read_line(&mut ack).unwrap();
        assert_eq!(ack.trim(), "next", "run through scripts/benchmark_relay.py");
    }
}

struct Pair {
    viewer: RvpConnection,
    host: RvpConnection,
    _bridge: ClientBridge,
    task: tokio::task::JoinHandle<Result<()>>,
    endpoints: [quinn::Endpoint; 2],
}
impl Drop for Pair {
    fn drop(&mut self) {
        self.task.abort();
        for endpoint in &self.endpoints {
            endpoint.close(0u32.into(), b"benchmark done");
        }
    }
}

async fn pair(
    host_cfg: &TunnelConfig,
    host_id: &DeviceIdentity,
    viewer_id: &DeviceIdentity,
) -> Result<Pair> {
    let (host_ep, _) = make_server_endpoint(
        "127.0.0.1:0".parse()?,
        host_id,
        PinState::new([viewer_id.fingerprint], false),
    )?;
    let tunnel = client::connect_host(host_cfg, host_id).await?;
    let address = host_ep.local_addr()?;
    let task = tokio::spawn(tunnel.run(address));
    let cfg = TunnelConfig {
        token: "22".repeat(32),
        ..host_cfg.clone()
    };
    let bridge = ClientBridge::start(&cfg, viewer_id).await?;
    let (viewer_ep, _) = make_client_endpoint(
        "127.0.0.1:0".parse()?,
        viewer_id,
        PinState::new([host_id.fingerprint], false),
    )?;
    let (viewer, host) = tokio::try_join!(
        async { Ok::<_, anyhow::Error>(viewer_ep.connect(bridge.address, "removent")?.await?) },
        async { Ok::<_, anyhow::Error>(host_ep.accept().await.context("host stopped")?.await?) }
    )?;
    Ok(Pair {
        viewer: RvpConnection::new(viewer),
        host: RvpConnection::new(host),
        _bridge: bridge,
        task,
        endpoints: [viewer_ep, host_ep],
    })
}

async fn media(pair: &Pair, mbps: usize, seconds: u64) -> Result<(usize, Vec<f64>)> {
    // A persistent uni stream with 30 fps writes, like the host video sender.
    let frames = seconds as usize * 30;
    let frame_bytes = mbps * 1_000_000 / 8 / 30;
    let payload = vec![0x5a; frame_bytes];
    let traffic = async {
        tokio::try_join!(
            async {
                let mut timer = tokio::time::interval(Duration::from_secs_f64(1.0 / 30.0));
                timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let mut stream = pair.host.open_media_stream().await?;
                for _ in 0..frames {
                    timer.tick().await;
                    stream.write_all(&payload).await?;
                }
                stream.finish()?;
                stream.stopped().await?;
                Ok::<_, anyhow::Error>(())
            },
            async {
                let mut bytes = 0;
                let mut stream = pair.viewer.accept_media_stream().await?;
                let mut data = vec![0; frame_bytes];
                for _ in 0..frames {
                    stream.read_exact(&mut data).await?;
                    ensure!(data == payload, "media payload mismatch");
                    bytes += data.len();
                }
                Ok::<_, anyhow::Error>(bytes)
            }
        )
    };
    // Control round trips concurrently with video, including return traffic.
    let ping = async {
        let mut latencies = Vec::new();
        let (mut tx, mut rx) = pair.viewer.inner().open_bi().await?;
        for _ in 0..seconds * 10 {
            let now = Instant::now();
            tx.write_all(b"12345678").await?;
            let mut reply = [0; 8];
            rx.read_exact(&mut reply).await?;
            ensure!(&reply == b"12345678", "control payload mismatch");
            latencies.push(now.elapsed().as_secs_f64() * 1000.);
            tokio::time::sleep_until((now + Duration::from_millis(100)).into()).await;
        }
        tx.finish()?;
        Ok::<_, anyhow::Error>(latencies)
    };
    let echo = async {
        let (mut tx, mut rx) = pair.host.inner().accept_bi().await?;
        for _ in 0..seconds * 10 {
            let mut data = [0; 8];
            rx.read_exact(&mut data).await?;
            tx.write_all(&data).await?;
        }
        tx.finish()?;
        Ok::<_, anyhow::Error>(())
    };
    let (((), bytes), latencies, ()) = tokio::try_join!(traffic, ping, echo)?;
    Ok((bytes, latencies))
}

async fn idle(pid: u32, name: &str, seconds: u64) {
    event(json!({"event":"start", "pid":pid, "phase":name}));
    tokio::time::sleep(Duration::from_secs(seconds)).await;
    event(json!({"event":"end", "phase":name}));
}

#[tokio::main(worker_threads = 4)]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() == 4,
        "resource_benchmark RELAY_BINARY quic|websocket SECONDS_PER_PHASE"
    );
    let websocket = match args[2].as_str() {
        "quic" => false,
        "websocket" => true,
        _ => anyhow::bail!("invalid transport"),
    };
    let seconds = args[3].parse::<u64>()?;
    ensure!(
        (2..=300).contains(&seconds),
        "phase duration must be 2..300 seconds"
    );
    let dir = tempfile::tempdir()?;
    let id = |name: &str| {
        identity::load_or_create(
            &DataPaths {
                root: dir.path().join(name),
            },
            name,
        )
    };
    let relay_id = id("relay")?;
    let host_id = id("host")?;
    let viewer_id = id("viewer")?;
    let address = if websocket {
        std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?
    } else {
        std::net::UdpSocket::bind("127.0.0.1:0")?.local_addr()?
    };
    let rooms: Vec<_> = (0..32).map(|i| json!({
        "name":format!("bench-{i}"),
        "host_token_sha256":hex::encode(removent_relay::config::token_hash(&"11".repeat(32)).unwrap()),
        "client_token_sha256":hex::encode(removent_relay::config::token_hash(&"22".repeat(32)).unwrap()),
        "host_public_keys":[hex::encode(host_id.verifying_key().as_bytes())],
        "client_public_keys":[hex::encode(viewer_id.verifying_key().as_bytes())]
    })).collect();
    let config = json!({"listen":address, "identity_dir":dir.path().join("relay"), "max_connections":128,
        "max_clients_per_room":4, "max_bytes_per_second":50_000_000u64, "allowed_cidrs":["127.0.0.0/8"], "rooms":rooms});
    let config_path = dir.path().join("relay.toml");
    std::fs::write(&config_path, toml::to_string(&config)?)?;
    let mut process = Command::new(&args[1])
        .arg(if websocket {
            "serve-websocket"
        } else {
            "serve"
        })
        .arg(&config_path)
        .env("REMOVENT_RELAY_IDLE_SECS", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()?;
    let pid = process.id().context("missing relay pid")?;
    let mut logs = BufReader::new(process.stdout.take().unwrap()).lines();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let line = logs
                .next_line()
                .await?
                .context("relay exited before readiness")?;
            if line.contains("listening") {
                break;
            }
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    let log_task = tokio::spawn(async move { while let Ok(Some(_)) = logs.next_line().await {} });
    let base = TunnelConfig {
        server: format!("removent://{address}"),
        transport: if websocket {
            RelayTransport::WebSocket
        } else {
            RelayTransport::Quic
        },
        insecure_loopback: websocket,
        server_fingerprint: if websocket {
            String::new()
        } else {
            relay_id.fingerprint_hex()
        },
        host_fingerprint: host_id.fingerprint_hex(),
        room: "bench-0".into(),
        token: "11".repeat(32),
    };
    idle(pid, "idle-no-connections", seconds).await;
    let host = client::connect_host(&base, &host_id).await?;
    idle(pid, "idle-one-host", seconds).await;
    drop(host);
    tokio::time::sleep(Duration::from_secs(1)).await;
    for count in [1, 4] {
        let mut pairs = Vec::new();
        for i in 0..count {
            let cfg = TunnelConfig {
                room: format!("bench-{i}"),
                ..base.clone()
            };
            pairs.push(pair(&cfg, &host_id, &viewer_id).await?);
        }
        idle(pid, &format!("idle-{count}-pairs"), seconds).await;
        for mbps in if count == 1 { [8, 24] } else { [12, 24] } {
            // Warm up outside the resource measurement window.
            futures::future::try_join_all(pairs.iter().map(|p| media(p, mbps, 2))).await?;
            let phase = format!("{count}-pairs-{mbps}mbps-each");
            event(json!({"event":"start", "pid":pid, "phase":phase}));
            let started = Instant::now();
            let outputs = tokio::time::timeout(
                Duration::from_secs(seconds * 4 + 10),
                futures::future::try_join_all(pairs.iter().map(|p| media(p, mbps, seconds))),
            )
            .await??;
            let bytes: usize = outputs.iter().map(|o| o.0).sum();
            let mut rtt: Vec<_> = outputs.into_iter().flat_map(|o| o.1).collect();
            rtt.sort_by(f64::total_cmp);
            event(
                json!({"event":"end", "phase":phase, "payload_bytes":bytes, "delivered_mbps":bytes as f64 * 8. / started.elapsed().as_secs_f64() / 1e6,
                "rtt_p50_ms":rtt[rtt.len()/2], "rtt_p95_ms":rtt[rtt.len()*95/100], "rtt_p99_ms":rtt[rtt.len()*99/100]}),
            );
        }
        drop(pairs);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let mut hosts = Vec::new();
    for i in 0..32 {
        let cfg = TunnelConfig {
            room: format!("bench-{i}"),
            ..base.clone()
        };
        hosts.push(client::connect_host(&cfg, &host_id).await?);
    }
    idle(pid, "idle-32-hosts", seconds).await;
    drop(hosts);
    tokio::time::sleep(Duration::from_secs(2)).await;
    idle(pid, "idle-after-disconnect", seconds).await;
    process.kill().await?;
    process.wait().await?;
    log_task.await?;
    Ok(())
}
