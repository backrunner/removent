//! Local RVP control-path RTT benchmark over a real mutual-TLS QUIC connection.
//!
//! Run with:
//!   cargo run --release -p removent-net --example rtt_benchmark

use futures::{SinkExt, StreamExt};
use removent_core::{DataPaths, identity};
use removent_net::{
    ControlItem, PinState, RvpConnection, make_client_endpoint, make_server_endpoint,
};
use removent_proto::{
    Caps, ControlMsg, HandshakeClient, HandshakeServer, Hello, MAGIC, PROTO_VERSION,
};
use std::time::{Duration, Instant};

const WARMUP: usize = 20;
const SAMPLES: usize = 500;

fn percentile(sorted: &[Duration], p: usize, q: usize) -> Duration {
    let index = (sorted.len().saturating_sub(1) * p / q).min(sorted.len().saturating_sub(1));
    sorted[index]
}

fn identity_for(name: &str) -> Result<(removent_core::DeviceIdentity, tempfile::TempDir), String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let paths = DataPaths {
        root: dir.path().to_path_buf(),
    };
    let id = identity::load_or_create(&paths, name).map_err(|e| e.to_string())?;
    Ok((id, dir))
}

async fn ping_once(
    sink: &mut removent_net::ControlSink,
    source: &mut removent_net::ControlSource,
    seq: u64,
) -> Result<Duration, String> {
    let started = Instant::now();
    sink.send(ControlMsg::Ping { ts_us: seq })
        .await
        .map_err(|e| e.to_string())?;
    loop {
        let item = tokio::time::timeout(Duration::from_secs(3), source.next())
            .await
            .map_err(|_| "Pong timed out".to_string())?
            .ok_or_else(|| "control stream closed".to_string())?
            .map_err(|e| e.to_string())?;
        if let ControlItem::Msg(msg) = item
            && matches!(*msg, ControlMsg::Pong { ts_us } if ts_us == seq)
        {
            return Ok(started.elapsed());
        }
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), String> {
    let (host_id, _host_dir) = identity_for("RTT benchmark host")?;
    let (client_id, _client_dir) = identity_for("RTT benchmark client")?;
    let (server_endpoint, _) = make_server_endpoint(
        "127.0.0.1:0"
            .parse()
            .map_err(|e: std::net::AddrParseError| e.to_string())?,
        &host_id,
        PinState::new([], true),
    )
    .map_err(|e| e.to_string())?;
    let server_addr = server_endpoint.local_addr().map_err(|e| e.to_string())?;
    let server = tokio::spawn(async move {
        let incoming = server_endpoint
            .accept()
            .await
            .ok_or_else(|| "server endpoint closed".to_string())?;
        let conn = RvpConnection::new(incoming.await.map_err(|e| e.to_string())?);
        let (_, mut sink, mut source) = conn
            .accept_handshake(|_| HandshakeServer {
                proto_version: PROTO_VERSION,
                feature_bits: 0,
                device_name: "RTT benchmark host".into(),
                resume_accepted: None,
                peer_known: true,
            })
            .await
            .map_err(|e| e.to_string())?;
        while let Some(item) = source.next().await {
            let item = item.map_err(|e| e.to_string())?;
            match item {
                ControlItem::Msg(msg) => match *msg {
                    ControlMsg::Ping { ts_us } => sink
                        .send(ControlMsg::Pong { ts_us })
                        .await
                        .map_err(|e| e.to_string())?,
                    ControlMsg::SessionEnd { .. } => break,
                    _ => {}
                },
                ControlItem::Skipped => {}
            }
        }
        Ok::<(), String>(())
    });

    let (client_endpoint, _) = make_client_endpoint(
        "127.0.0.1:0"
            .parse()
            .map_err(|e: std::net::AddrParseError| e.to_string())?,
        &client_id,
        PinState::new([], true),
    )
    .map_err(|e| e.to_string())?;
    let conn = RvpConnection::new(
        client_endpoint
            .connect(server_addr, "removent")
            .map_err(|e| e.to_string())?
            .await
            .map_err(|e| e.to_string())?,
    );
    let (_, mut sink, mut source) = conn
        .connect_handshake(HandshakeClient {
            magic: MAGIC,
            proto_version: PROTO_VERSION,
            feature_bits: 0,
            hello: Hello {
                app_version: env!("CARGO_PKG_VERSION").into(),
                device_name: "RTT benchmark client".into(),
                os_version: std::env::consts::OS.into(),
                caps: Caps::default(),
                resume_token: None,
            },
        })
        .await
        .map_err(|e| e.to_string())?;

    for n in 0..WARMUP {
        ping_once(&mut sink, &mut source, n as u64).await?;
    }
    let mut timings = Vec::with_capacity(SAMPLES);
    for n in 0..SAMPLES {
        timings.push(ping_once(&mut sink, &mut source, (WARMUP + n) as u64).await?);
    }
    timings.sort_unstable();
    let average = timings.iter().sum::<Duration>() / timings.len() as u32;
    println!(
        "RVP control RTT loopback: samples={} min={:.3}ms avg={:.3}ms p50={:.3}ms p95={:.3}ms p99={:.3}ms max={:.3}ms quic_estimate={:.3}ms build={}",
        timings.len(),
        timings[0].as_secs_f64() * 1_000.0,
        average.as_secs_f64() * 1_000.0,
        percentile(&timings, 50, 100).as_secs_f64() * 1_000.0,
        percentile(&timings, 95, 100).as_secs_f64() * 1_000.0,
        percentile(&timings, 99, 100).as_secs_f64() * 1_000.0,
        timings[timings.len() - 1].as_secs_f64() * 1_000.0,
        conn.rtt_estimate(),
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
    );
    sink.send(ControlMsg::SessionEnd {
        reason: removent_proto::EndReason::ClientClosed,
    })
    .await
    .map_err(|e| e.to_string())?;
    server.await.map_err(|e| e.to_string())??;
    Ok(())
}
