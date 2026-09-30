//! Cloudflare-compatible WSS carrier. The payload remains end-to-end RVP/QUIC.
//! TLS terminates at the edge; native device authentication stays inside RVP.
mod client;
mod server;
pub use client::{connect, host_loop, start_client};
pub use server::serve;

use anyhow::{Result, bail, ensure};
use bytes::{BufMut, Bytes, BytesMut};
use futures::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::mpsc,
};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, protocol::WebSocketConfig},
};
use tokio_util::sync::CancellationToken;

pub const PROTOCOL: &str = "removent-relay.ws.v1";
pub const PATH: &str = "/v1/tunnel";
pub const MAX_MESSAGE: usize = 8 + crate::wire::MAX_PACKET;
// Outside the inner QUIC scheduler: keep this FIFO to a short burst, so
// prioritized input cannot sit behind a quarter-megabyte of encrypted video.
const QUEUE: usize = 16;
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(90);

pub(crate) fn limits() -> WebSocketConfig {
    WebSocketConfig::default()
        // Tunnel frames are <= 2056 bytes. 16 KiB still batches several frames
        // without reserving tungstenite's 128 KiB default for every idle peer.
        .read_buffer_size(16 * 1024)
        .max_message_size(Some(MAX_MESSAGE))
        .max_frame_size(Some(MAX_MESSAGE))
        .max_write_buffer_size(64 * 1024)
        .write_buffer_size(16 * 1024)
}

#[derive(Clone)]
pub(crate) struct Sender {
    tx: mpsc::Sender<Bytes>,
    pub stop: CancellationToken,
}
impl Sender {
    pub async fn raw(&self, packet: Bytes) -> Result<()> {
        ensure!(!self.stop.is_cancelled(), "WebSocket relay disconnected");
        // Bound both memory and queuing delay; inner QUIC recovers dropped UDP.
        if let Ok(result) =
            tokio::time::timeout(Duration::from_millis(50), self.tx.send(packet)).await
        {
            result.map_err(|_| anyhow::anyhow!("WebSocket relay disconnected"))?;
        }
        Ok(())
    }
    pub async fn packet(&self, route: u64, packet: &[u8]) -> Result<()> {
        ensure!(
            !packet.is_empty() && packet.len() <= crate::wire::MAX_PACKET,
            "Invalid tunnel packet length"
        );
        let mut bytes = BytesMut::with_capacity(8 + packet.len());
        bytes.put_u64(route);
        bytes.extend_from_slice(packet);
        self.raw(bytes.freeze()).await
    }
}

pub struct Tunnel {
    pub route: u64,
    pub(crate) sender: Sender,
    incoming: mpsc::Receiver<Bytes>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Tunnel {
    fn drop(&mut self) {
        self.sender.stop.cancel();
        self.task.abort();
    }
}
impl Tunnel {
    pub async fn send_packet(&self, route: u64, packet: &[u8]) -> Result<()> {
        self.sender.packet(route, packet).await
    }
    pub(crate) fn start<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
        socket: WebSocketStream<S>,
        route: u64,
    ) -> Self {
        let (tx, outgoing) = mpsc::channel(QUEUE);
        let (incoming_tx, incoming) = mpsc::channel(QUEUE);
        let stop = CancellationToken::new();
        let stopped = stop.clone();
        let task = tokio::spawn(async move {
            let _ = pump(socket, outgoing, incoming_tx, &stopped).await;
            stopped.cancel();
        });
        Self {
            route,
            sender: Sender { tx, stop },
            incoming,
            task,
        }
    }
    pub async fn receive(&mut self) -> Result<Bytes> {
        self.incoming
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("WebSocket relay disconnected"))
    }
}

pub(crate) fn parse_packet(bytes: &Bytes) -> Option<(u64, Bytes)> {
    if !(9..=MAX_MESSAGE).contains(&bytes.len()) {
        return None;
    }
    Some((
        u64::from_be_bytes(bytes[..8].try_into().ok()?),
        bytes.slice(8..),
    ))
}

async fn pump<S: AsyncRead + AsyncWrite + Unpin>(
    socket: WebSocketStream<S>,
    mut outgoing: mpsc::Receiver<Bytes>,
    incoming: mpsc::Sender<Bytes>,
    stop: &CancellationToken,
) -> Result<()> {
    // Split polling, not connections: a congested video write must not suspend
    // reading the opposite direction's input/ACKs for the five-second deadline.
    let (mut sink, mut source) = socket.split();
    let (pong_tx, mut pong_rx) = mpsc::channel(4);
    let read = async {
        loop {
            let packet = tokio::time::timeout(Duration::from_secs(20), source.next()).await?;
            let Some(packet) = packet else {
                return Ok::<_, anyhow::Error>(());
            };
            match packet? {
                Message::Binary(bytes) => {
                    ensure!(parse_packet(&bytes).is_some(), "Invalid relay frame");
                    if let Ok(result) =
                        tokio::time::timeout(Duration::from_millis(50), incoming.send(bytes)).await
                    {
                        result?;
                    }
                }
                Message::Ping(bytes) => {
                    pong_tx.try_send(bytes)?;
                }
                Message::Pong(_) => {}
                Message::Close(_) => return Ok(()),
                _ => bail!("Binary relay frames required"),
            }
        }
    };
    let write = async {
        let mut heartbeat = tokio::time::interval(Duration::from_secs(5));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            let message = tokio::select! {
                biased;
                pong = pong_rx.recv() => {
                    let Some(bytes) = pong else { return Ok::<_, anyhow::Error>(()); };
                    Message::Pong(bytes)
                }
                _ = heartbeat.tick() => Message::Ping(Bytes::new()),
                packet = outgoing.recv() => {
                    let Some(packet) = packet else { return Ok(()); };
                    Message::Binary(packet)
                }
            };
            tokio::time::timeout(Duration::from_secs(5), sink.send(message)).await??;
        }
    };
    tokio::select! {
        _ = stop.cancelled() => Ok(()),
        result = read => result,
        result = write => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::tungstenite::protocol::Role;

    #[tokio::test]
    async fn blocked_downstream_write_does_not_block_upstream_input() {
        let (a, b) = tokio::io::duplex(128);
        let a = WebSocketStream::from_raw_socket(a, Role::Server, Some(limits())).await;
        let mut peer = WebSocketStream::from_raw_socket(b, Role::Client, Some(limits())).await;
        let mut tunnel = Tunnel::start(a, 1);
        // Peer deliberately does not consume any downstream video.
        tunnel.send_packet(1, &[7; 2048]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(25)).await;
        let mut upstream = vec![0; 9];
        upstream[7] = 1;
        upstream[8] = 42;
        peer.send(Message::Binary(upstream.clone().into()))
            .await
            .unwrap();
        let received = tokio::time::timeout(Duration::from_millis(500), tunnel.receive())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&received[..], &upstream);
    }
}
