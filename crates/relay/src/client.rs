use crate::{
    AUTH_TIMEOUT, client_endpoint,
    config::{Role, TunnelConfig, decode_secret},
    server::Hello,
    wire,
};
use anyhow::{Context, Result, ensure};
use removent_core::DeviceIdentity;
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::net::UdpSocket;

pub struct Tunnel {
    pub endpoint: quinn::Endpoint,
    pub connection: quinn::Connection,
    pub route: u64,
}
impl Drop for Tunnel {
    fn drop(&mut self) {
        self.endpoint.close(0u32.into(), b"tunnel closed");
    }
}

pub async fn connect(cfg: &TunnelConfig, role: Role, identity: &DeviceIdentity) -> Result<Tunnel> {
    cfg.validate()?;
    tokio::time::timeout(AUTH_TIMEOUT, async {
        let addr = tokio::net::lookup_host(cfg.endpoint()?.authority())
            .await?
            .next()
            .context("Relay address not found")?;
        let endpoint = client_endpoint(addr, identity, decode_secret(&cfg.server_fingerprint)?)?;
        let connection = endpoint.connect(addr, "removent-relay")?.await?;
        let (mut send, mut recv) = connection.open_bi().await?;
        let auth = serde_json::to_vec(&Hello {
            room: cfg.room.clone(),
            token: cfg.token.clone(),
            role,
            public_key: hex::encode(identity.verifying_key().as_bytes()),
        })?;
        send.write_all(&(auth.len() as u16).to_be_bytes()).await?;
        send.write_all(&auth).await?;
        let mut nonce = [0; 32];
        recv.read_exact(&mut nonce).await?;
        send.write_all(&crate::auth::sign_challenge(
            identity, &cfg.room, role, &nonce,
        ))
        .await?;
        send.finish()?;
        let mut ack = [0; 8];
        recv.read_exact(&mut ack)
            .await
            .context("Relay authentication failed or host unavailable")?;
        Ok::<_, anyhow::Error>(Tunnel {
            endpoint,
            connection,
            route: u64::from_be_bytes(ack),
        })
    })
    .await
    .context("Relay connection timed out")?
}

/// Holds the local tunnel for a native desktop connection. Dropping the viewer
/// cancels the task and closes the outer connection, including failed pairing.
pub struct ClientBridge {
    pub address: SocketAddr,
    task: tokio::task::JoinHandle<Result<()>>,
}
impl Drop for ClientBridge {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl ClientBridge {
    pub async fn start(cfg: &TunnelConfig, identity: &DeviceIdentity) -> Result<Self> {
        if cfg.is_websocket() {
            let (address, task) = crate::websocket::start_client(cfg, identity).await?;
            return Ok(Self { address, task });
        }
        let tunnel = connect(cfg, Role::Client, identity).await?;
        let socket = UdpSocket::bind("127.0.0.1:0").await?;
        let address = socket.local_addr()?;
        let task = tokio::spawn(client_loop(tunnel, socket));
        Ok(Self { address, task })
    }
}

pub enum HostTunnel {
    Quic(Tunnel),
    WebSocket(crate::websocket::Tunnel),
}
impl HostTunnel {
    pub async fn run(self, target: SocketAddr) -> Result<()> {
        match self {
            Self::Quic(tunnel) => host_loop(tunnel, target).await,
            Self::WebSocket(tunnel) => crate::websocket::host_loop(tunnel, target).await,
        }
    }
}
pub async fn connect_host(cfg: &TunnelConfig, identity: &DeviceIdentity) -> Result<HostTunnel> {
    if cfg.is_websocket() {
        Ok(HostTunnel::WebSocket(
            crate::websocket::connect(cfg, Role::Host, identity).await?,
        ))
    } else {
        Ok(HostTunnel::Quic(connect(cfg, Role::Host, identity).await?))
    }
}

async fn client_loop(tunnel: Tunnel, socket: UdpSocket) -> Result<()> {
    let mut peer = None;
    let mut buffer = [0u8; wire::MAX_PACKET + 1];
    let mut assembler = wire::Assembler::default();
    let mut sequence = 0;
    loop {
        tokio::select! {
            packet = socket.recv_from(&mut buffer) => {
                let (n, addr) = packet?;
                if peer.is_some_and(|p| p != addr) || n > wire::MAX_PACKET { continue; }
                peer = Some(addr);
                wire::send(&tunnel.connection, tunnel.route, &mut sequence, &buffer[..n]).await?;
            }
            packet = tunnel.connection.read_datagram() => {
                if let Some((route, packet)) = assembler.receive(packet?)
                    && route == tunnel.route && let Some(peer) = peer {
                    socket.send_to(&packet, peer).await?;
                }
            }
        }
    }
}

struct HostRoute {
    socket: Arc<UdpSocket>,
    last: Instant,
    reader: tokio::task::JoinHandle<()>,
}
impl Drop for HostRoute {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

/// Only forwards to the fixed local RVP listener, never to a client-supplied
/// address. Each controller gets its own source port for QUIC peer separation.
pub async fn host_loop(tunnel: Tunnel, target: SocketAddr) -> Result<()> {
    ensure!(
        target.ip().is_loopback() && target.port() != 0,
        "Host target must be local RVP"
    );
    let mut routes: HashMap<u64, HostRoute> = HashMap::new();
    let mut assembler = wire::Assembler::default();
    let mut reap = tokio::time::interval(Duration::from_secs(5));
    loop {
        tokio::select! {
            _ = reap.tick() => { routes.retain(|_, r| r.last.elapsed() < Duration::from_secs(30)); }
            packet = tunnel.connection.read_datagram() => {
                let Some((route, packet)) = assembler.receive(packet?) else { continue; };
                if route == 0 { continue; }
                if !routes.contains_key(&route) {
                    if routes.len() >= 64 { continue; }
                    let socket = Arc::new(UdpSocket::bind(if target.is_ipv4() { "127.0.0.1:0" } else { "[::1]:0" }).await?);
                    socket.connect(target).await?;
                    let reply = socket.clone();
                    let conn = tunnel.connection.clone();
                    let reader = tokio::spawn(async move {
                        let mut buffer = [0; wire::MAX_PACKET + 1];
                        let mut sequence = 0;
                        while let Ok(n) = reply.recv(&mut buffer).await {
                            if n <= wire::MAX_PACKET && wire::send(&conn, route, &mut sequence, &buffer[..n]).await.is_err() { break; }
                        }
                    });
                    routes.insert(route, HostRoute { socket, last: Instant::now(), reader });
                }
                let local = routes.get_mut(&route).unwrap();
                local.last = Instant::now();
                // An ICMP error while the RVP listener is restarting is transient.
                let _ = local.socket.send(&packet).await;
            }
        }
    }
}
