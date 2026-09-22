use crate::{
    AUTH_TIMEOUT,
    auth::{self, Policy},
    config::{Role, ServerConfig, decode_secret},
    wire,
};
use anyhow::{Result, bail, ensure};
use quinn::{Connection, Endpoint};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Hello {
    pub room: String,
    pub token: String,
    pub role: Role,
    pub public_key: String,
}

struct RoomState {
    host_auth: Policy,
    client_auth: Policy,
    routes: Mutex<Routes>,
}
#[derive(Default)]
struct Routes {
    host: Option<Connection>,
    clients: HashMap<u64, Connection>,
}

/// Per-connection token bucket; no unbounded IP/credential rate-limit tables.
pub(crate) struct Budget {
    rate: f64,
    available: f64,
    last: Instant,
}
impl Budget {
    pub(crate) fn new(rate: u64) -> Self {
        Self {
            rate: rate as f64,
            available: rate as f64,
            last: Instant::now(),
        }
    }
    pub(crate) fn allow(&mut self, bytes: usize) -> bool {
        let now = Instant::now();
        self.available = (self.available + now.duration_since(self.last).as_secs_f64() * self.rate)
            .min(self.rate);
        self.last = now;
        if self.available < bytes as f64 {
            return false;
        }
        self.available -= bytes as f64;
        true
    }
}

struct Registration {
    room: Arc<RoomState>,
    role: Role,
    id: u64,
    conn: Connection,
}
impl Drop for Registration {
    fn drop(&mut self) {
        self.conn.close(0u32.into(), b"relay session ended");
        let mut routes = self.room.routes.lock().unwrap();
        match self.role {
            Role::Host => {
                routes.host = None;
                for (_, c) in routes.clients.drain() {
                    c.close(0u32.into(), b"host offline");
                }
            }
            Role::Client => {
                routes.clients.remove(&self.id);
            }
        }
    }
}

pub async fn serve(
    endpoint: Endpoint,
    config: ServerConfig,
    stop: CancellationToken,
) -> Result<()> {
    config.validate()?;
    let rooms = Arc::new(
        config
            .rooms
            .iter()
            .map(|r| {
                Ok((
                    r.name.clone(),
                    Arc::new(RoomState {
                        host_auth: Policy::new(&r.host_token_sha256, &r.host_public_keys)?,
                        client_auth: Policy::new(&r.client_token_sha256, &r.client_public_keys)?,
                        routes: Mutex::new(Routes::default()),
                    }),
                ))
            })
            .collect::<Result<HashMap<_, _>>>()?,
    );
    let permits = Arc::new(Semaphore::new(config.max_connections));
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            _ = stop.cancelled() => break,
            _ = tasks.join_next(), if !tasks.is_empty() => {},
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else { break; };
                // Reject before TLS allocation; do not trust any application headers.
                if !config.allows_ip(incoming.remote_address().ip()) { incoming.refuse(); continue; }
                // QUIC Retry validates return routability before allocating TLS state.
                if !incoming.remote_address_validated() { let _ = incoming.retry(); continue; }
                let Ok(permit) = permits.clone().try_acquire_owned() else { incoming.refuse(); continue; };
                let rooms = rooms.clone();
                let limit = config.max_clients_per_room;
                let rate = config.max_bytes_per_second;
                tasks.spawn(async move {
                    let _permit = permit;
                    let work = async {
                        let conn = incoming.await?;
                        let (mut send, mut recv) = conn.accept_bi().await?;
                        let mut size = [0; 2];
                        recv.read_exact(&mut size).await?;
                        let size = u16::from_be_bytes(size) as usize;
                        ensure!(size <= 1024, "Invalid hello");
                        let mut bytes = vec![0; size];
                        recv.read_exact(&mut bytes).await?;
                        let hello: Hello = serde_json::from_slice(&bytes)?;
                        let key = decode_secret(&hello.public_key)?;
                        let room = rooms.get(&hello.room).ok_or_else(|| anyhow::anyhow!("Unauthorized"))?.clone();
                        let policy = match hello.role { Role::Host => &room.host_auth, Role::Client => &room.client_auth };
                        policy.authorize(&hello.token, &key)?;
                        let nonce: [u8; 32] = rand::random();
                        send.write_all(&nonce).await?;
                        let mut proof = [0; 64];
                        recv.read_exact(&mut proof).await?;
                        auth::verify(&key, &auth::challenge_message(&hello.room, hello.role, &nonce), &proof)?;
                        let id = {
                            let mut routes = room.routes.lock().unwrap();
                            match hello.role {
                                Role::Host => {
                                    ensure!(routes.host.is_none(), "Host already online");
                                    routes.host = Some(conn.clone());
                                    0
                                }
                                Role::Client => {
                                    ensure!(routes.host.is_some(), "Host offline");
                                    ensure!(routes.clients.len() < limit, "Room full");
                                    let mut id = rand::random::<u64>();
                                    while id == 0 || routes.clients.contains_key(&id) { id = rand::random(); }
                                    routes.clients.insert(id, conn.clone());
                                    id
                                }
                            }
                        };
                        let registration = Registration { room, role: hello.role, id, conn };
                        send.write_all(&id.to_be_bytes()).await?;
                        send.finish()?;
                        Ok::<_, anyhow::Error>(registration)
                    };
                    // Includes TLS, reading auth, and acknowledging registration.
                    if let Ok(Ok(registration)) = tokio::time::timeout(AUTH_TIMEOUT, work).await {
                        let _ = forward(&registration, rate).await;
                    }
                    // Never log submitted tokens or deserialization errors.
                });
            }
        }
    }
    endpoint.close(0u32.into(), b"relay shutdown");
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    Ok(())
}

async fn forward(reg: &Registration, rate: u64) -> Result<()> {
    let mut budget = Budget::new(rate);
    // The host connection is stable for this controller's lifetime.
    let host = reg.room.routes.lock().unwrap().host.clone();
    loop {
        let packet = reg.conn.read_datagram().await?;
        if packet.len() > wire::MAX_PACKET + wire::HEADER || !budget.allow(packet.len()) {
            continue;
        }
        let Some(id) = wire::route(&packet) else {
            continue;
        };
        let target = match reg.role {
            Role::Client => {
                if id != reg.id {
                    bail!("Invalid route");
                }
                host.clone()
            }
            Role::Host => reg.room.routes.lock().unwrap().clients.get(&id).cloned(),
        };
        if let Some(target) = target {
            // Bytes is reference counted. No decoding, transcoding, per-packet
            // task, or application payload copy on the relay's data path.
            let _ = wire::forward(&target, packet).await;
        }
    }
}
