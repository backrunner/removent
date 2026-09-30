//! RVP input delivery under a real encrypted link with synthetic media load.
//! No capture or OS input injection. Run a release build; output is CSV.
//! cargo run --release -p removent-host --example wan_latency -- quic 12
#[path = "wan/link.rs"]
mod link;
use anyhow::{Context, Result, ensure};
use futures::{SinkExt, StreamExt};
use link::{Link, Proxy};
use removent_core::{
    AdaptationController, DataPaths, QualityPreset, QualityState, identity,
    removent_uri::RelayTransport,
};
use removent_host::{
    delivery::DeliveryHealth,
    input_sink::InputSink,
    session::{ControlPumpDeps, spawn_control_pump},
};
use removent_net::{
    ControlItem, PinState, RvpConnection, make_client_endpoint, make_server_endpoint,
};
use removent_proto::{
    Caps, ControlMsg, HandshakeClient, HandshakeServer, Hello, KeyKind, KeyModifiers, MouseKind,
    ScrollPhase,
};
use removent_relay::{
    client::{self, ClientBridge},
    config::{Room, ServerConfig, TunnelConfig, token_hash},
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Inputs {
    starts: Mutex<Vec<Instant>>,
    ages: Mutex<Vec<f64>>,
    downs: AtomicU64,
    ups: AtomicU64,
}
impl InputSink for Inputs {
    fn mouse(&self, _: u64, _: f32, _: f32, _: u8, _: MouseKind) -> Result<(), String> {
        Ok(())
    }
    fn scroll(&self, _: u64, _: f32, _: f32, _: ScrollPhase) -> Result<(), String> {
        Ok(())
    }
    fn key(&self, key: u16, _: KeyModifiers, kind: KeyKind, _: Option<char>) -> Result<(), String> {
        if kind == KeyKind::Down {
            self.downs.fetch_add(1, Ordering::Relaxed);
            self.ages.lock().unwrap().push(
                self.starts.lock().unwrap()[key as usize]
                    .elapsed()
                    .as_secs_f64()
                    * 1000.,
            );
        } else if kind == KeyKind::Up {
            self.ups.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }
}
fn p(values: &[f64], percent: usize) -> f64 {
    if values.is_empty() {
        return f64::NAN;
    }
    let mut values = values.to_vec();
    values.sort_by(f64::total_cmp);
    values[(values.len() - 1) * percent / 100]
}

#[allow(clippy::too_many_arguments)]
async fn run(
    carrier: &str,
    label: &str,
    rtt: u64,
    kbps: u64,
    loss: u32,
    stall: u64,
    seconds: u64,
    video: bool,
) -> Result<()> {
    let directory = tempfile::tempdir()?;
    let id = |name: &str| {
        identity::load_or_create(
            &DataPaths {
                root: directory.path().join(name),
            },
            name,
        )
    };
    let relay_id = id("relay")?;
    let host_id = id("host")?;
    let viewer_id = id("viewer")?;
    let stop = CancellationToken::new();
    let _guard = stop.clone().drop_guard();
    let mut tasks = tokio::task::JoinSet::new();
    let (host_ep, _) = make_server_endpoint(
        "127.0.0.1:0".parse()?,
        &host_id,
        PinState::new([viewer_id.fingerprint], false),
    )?;
    let mut proxies = Vec::new();
    let mut bridge = None;
    let hops = if carrier == "direct" { 2 } else { 4 };
    let link = Link {
        delay_ms: rtt / hops,
        kbps,
        loss_percent: loss,
        stall_ms: stall,
    };
    let target = if carrier == "direct" {
        proxies.push(Proxy::udp(host_ep.local_addr()?, link, 42).await?);
        proxies[0].address
    } else {
        let endpoint = removent_relay::server_endpoint("127.0.0.1:0".parse()?, &relay_id)?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let ws = carrier == "websocket";
        let address = if ws {
            listener.local_addr()?
        } else {
            endpoint.local_addr()?
        };
        let cfg = ServerConfig {
            listen: address,
            identity_dir: directory.path().join("relay"),
            max_connections: 8,
            max_clients_per_room: 4,
            max_bytes_per_second: 50_000_000,
            allowed_cidrs: vec![],
            updates: Default::default(),
            rooms: vec![Room {
                name: "wan".into(),
                host_token_sha256: hex::encode(token_hash(&"11".repeat(32))?),
                client_token_sha256: hex::encode(token_hash(&"22".repeat(32))?),
                host_public_keys: vec![hex::encode(host_id.verifying_key().as_bytes())],
                client_public_keys: vec![hex::encode(viewer_id.verifying_key().as_bytes())],
            }],
        };
        let cancel = stop.clone();
        tasks.spawn(async move {
            if ws {
                removent_relay::websocket::serve(listener, cfg, cancel, None).await
            } else {
                removent_relay::server::serve(endpoint, cfg, cancel).await
            }
        });
        for seed in [42, 137] {
            proxies.push(if ws {
                Proxy::tcp(address, link).await?
            } else {
                Proxy::udp(address, link, seed).await?
            });
        }
        let cfg = TunnelConfig {
            server: format!("removent://{}", proxies[0].address),
            transport: if ws {
                RelayTransport::WebSocket
            } else {
                RelayTransport::Quic
            },
            insecure_loopback: ws,
            server_fingerprint: if ws {
                String::new()
            } else {
                relay_id.fingerprint_hex()
            },
            host_fingerprint: host_id.fingerprint_hex(),
            room: "wan".into(),
            token: "11".repeat(32),
        };
        let tunnel = client::connect_host(&cfg, &host_id).await?;
        let target = host_ep.local_addr()?;
        tasks.spawn(tunnel.run(target));
        let cfg = TunnelConfig {
            server: format!("removent://{}", proxies[1].address),
            token: "22".repeat(32),
            ..cfg
        };
        bridge = Some(ClientBridge::start(&cfg, &viewer_id).await?);
        bridge.as_ref().unwrap().address
    };
    let (viewer_ep, _) = make_client_endpoint(
        "127.0.0.1:0".parse()?,
        &viewer_id,
        PinState::new([host_id.fingerprint], false),
    )?;
    let (viewer, host) = tokio::try_join!(
        async { Ok::<_, anyhow::Error>(viewer_ep.connect(target, "removent")?.await?) },
        async { Ok::<_, anyhow::Error>(host_ep.accept().await.context("host closed")?.await?) }
    )?;
    let viewer = RvpConnection::new(viewer);
    let host = RvpConnection::new(host);
    let (client_control, host_control) = tokio::join!(
        viewer.connect_handshake(HandshakeClient {
            magic: removent_proto::MAGIC,
            proto_version: 1,
            feature_bits: 0,
            hello: Hello {
                app_version: "bench".into(),
                device_name: "bench".into(),
                os_version: "bench".into(),
                caps: Caps::all(),
                resume_token: None
            }
        }),
        host.accept_handshake(|_| HandshakeServer {
            proto_version: 1,
            feature_bits: 0,
            device_name: "bench".into(),
            resume_accepted: None,
            peer_known: true
        })
    );
    let (_, mut tx, mut rx) = client_control?;
    let (_, sink, source) = host_control?;
    let inputs = Arc::new(Inputs::default());
    let health = Arc::new(DeliveryHealth::new(host.clone()));
    let quality = QualityState {
        bitrate_kbps: 12000,
        fps: 30,
        scale: 1.0,
    };
    let (quality_tx, quality_rx) = tokio::sync::watch::channel(quality);
    let (_commands, commands) = tokio::sync::mpsc::channel(64);
    let pump = spawn_control_pump(
        source,
        sink,
        ControlPumpDeps {
            conn: host.clone(),
            kf_tx: None,
            controller: Some(Arc::new(Mutex::new(AdaptationController::new(
                12000,
                30,
                QualityPreset::Auto,
            )))),
            window_ms: 250,
            input: Some(inputs.clone()),
            local_clip: None,
            quality_tx: Some(quality_tx),
            caps: Caps::all(),
            clip_state: None,
            cancel: stop.clone(),
            peer_fp: None,
            delivery: Some(health.clone()),
        },
        commands,
    );
    let pump_abort = pump.abort_handle();
    tasks.spawn(async move {
        pump.await?;
        Ok(())
    });
    let total = Arc::new(AtomicU64::new(0));
    if video {
        let mut send = host.open_media_stream().await?;
        let quality = quality_rx.clone();
        let stop_video = stop.clone();
        tasks.spawn(async move {
            let work = async {
                loop {
                    let q = *quality.borrow();
                    let bytes = vec![0x5a; q.bitrate_kbps as usize * 1000 / 8 / usize::from(q.fps)];
                    health.begin_write();
                    removent_net::session::write_media(&mut send, &bytes).await?;
                    health.end_write();
                    tokio::time::sleep(Duration::from_secs_f64(1. / f64::from(q.fps))).await;
                }
                #[allow(unreachable_code)]
                Ok::<_, anyhow::Error>(())
            };
            tokio::select! {_ = stop_video.cancelled()=>Ok(()),result=work=>result}
        });
        let viewer = viewer.clone();
        let bytes = total.clone();
        tasks.spawn(async move {
            let mut recv = viewer.accept_media_stream().await?;
            let mut buffer = [0; 16384];
            while let Some(n) = recv.read(&mut buffer).await? {
                bytes.fetch_add(n as u64, Ordering::Relaxed);
            }
            Ok(())
        });
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    let started = Instant::now();
    let first = total.load(Ordering::Relaxed);
    let scheduler_lag = Arc::new(AtomicU64::new(0));
    let lag = scheduler_lag.clone();
    let clock_stop = stop.clone();
    tasks.spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(10));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = clock_stop.cancelled() => return Ok(()),
                scheduled = tick.tick() => {
                    lag.fetch_max(scheduled.elapsed().as_micros() as u64, Ordering::Relaxed);
                }
            }
        }
    });
    // Fixed-rate input independent of responses: a slow Pong must not reduce
    // offered input load (coordinated omission would hide weak-link tails).
    let timing = inputs.clone();
    let sender = tokio::spawn(async move {
        let mut sent = 0u16;
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        while started.elapsed() < Duration::from_secs(seconds) {
            tick.tick().await;
            timing.starts.lock().unwrap().push(Instant::now());
            for kind in [KeyKind::Down, KeyKind::Up] {
                tx.send(ControlMsg::KeyEvent {
                    vk_code: sent,
                    modifiers: KeyModifiers::empty(),
                    kind,
                    unicode: None,
                })
                .await?;
            }
            tx.send(ControlMsg::Ping {
                ts_us: u64::from(sent),
            })
            .await?;
            sent += 1;
        }
        Ok::<_, anyhow::Error>((sent, tx))
    });
    // Poll replies independently, with two seconds of grace after the sample.
    let mut rtts = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds + 2);
    let mut delivered = None;
    let mut sample_end = tokio::time::interval(Duration::from_secs(seconds));
    sample_end.tick().await;
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => break,
            _ = sample_end.tick(), if delivered.is_none() => {
                delivered = Some((total.load(Ordering::Relaxed) - first, started.elapsed().as_secs_f64()));
            }
            item = rx.next() => {
                let item = item.context("control closed")??;
                if let ControlItem::Msg(msg) = item
                    && let ControlMsg::Pong { ts_us } = *msg {
                    rtts.push(inputs.starts.lock().unwrap()[ts_us as usize].elapsed().as_secs_f64() * 1000.);
                }
            }
        }
    }
    let (sent, _tx) = sender.await??;
    let timeouts = usize::from(sent) - rtts.len() + rtts.iter().filter(|&&t| t >= 2000.).count();
    let (delivered, elapsed) = delivered.unwrap_or_else(|| {
        (
            total.load(Ordering::Relaxed) - first,
            started.elapsed().as_secs_f64(),
        )
    });
    let ages = inputs.ages.lock().unwrap().clone();
    let downs = inputs.downs.load(Ordering::Relaxed);
    let ups = inputs.ups.load(Ordering::Relaxed);
    println!(
        "{carrier},{label},{rtt},{kbps},{loss},{stall},{sent},{downs},{ups},{timeouts},{:.2},{:.2},{:.2},{:.2},{:.3},{},{},{elapsed:.3},{:.2}",
        p(&ages, 50),
        p(&ages, 95),
        p(&rtts, 95),
        p(&rtts, 99),
        delivered as f64 * 8. / elapsed / 1e6,
        quality_rx.borrow().bitrate_kbps,
        proxies
            .iter()
            .map(|p| p.drops.load(Ordering::Relaxed))
            .sum::<u64>(),
        scheduler_lag.load(Ordering::Relaxed) as f64 / 1000.,
    );
    ensure!(
        downs == u64::from(sent) && ups == downs,
        "input transitions did not all arrive"
    );
    stop.cancel();
    pump_abort.abort();
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    drop(bridge);
    viewer_ep.close(0u32.into(), b"done");
    host_ep.close(0u32.into(), b"done");
    Ok(())
}

#[tokio::main(worker_threads = 4)]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let carrier = args.get(1).map(String::as_str).unwrap_or("quic");
    ensure!(
        ["direct", "quic", "websocket"].contains(&carrier),
        "carrier: direct|quic|websocket"
    );
    let seconds: u64 = args.get(2).map_or(Ok(12), |s| s.parse())?;
    ensure!((4..=120).contains(&seconds), "duration 4..120");
    println!(
        "carrier,scenario,nominal_rtt_ms,link_kbps,udp_loss_pct_per_leg,tcp_stall_ms,sent,down_received,up_received,timeouts,input_p50_ms,input_p95_ms,control_rtt_p95_ms,control_rtt_p99_ms,media_mbps,final_video_kbps,proxy_dropped_packets,sample_seconds,scheduler_lag_max_ms"
    );
    let ws = carrier == "websocket";
    for (label, rtt, kbps, loss, stall, video) in [
        ("idle", 80, 8000, 0, 0, false),
        ("video-overload", 80, 8000, 0, 0, true),
        (
            "loss-or-stall",
            80,
            8000,
            if ws { 0 } else { 2 },
            if ws { 200 } else { 0 },
            true,
        ),
        (
            "severe",
            150,
            2000,
            if ws { 0 } else { 5 },
            if ws { 300 } else { 0 },
            true,
        ),
    ] {
        tokio::time::timeout(
            Duration::from_secs(seconds * 4 + 40),
            run(carrier, label, rtt, kbps, loss, stall, seconds, video),
        )
        .await??;
    }
    Ok(())
}
