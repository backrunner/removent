use super::{PATH, PROTOCOL, Sender, Tunnel, limits, parse_packet};
use crate::{
    auth::{self, Policy},
    config::{Role, ServerConfig, decode_secret},
    server::Budget,
};
use anyhow::{Result, ensure};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use http_body_util::Full;
use hyper::{body::Incoming, server::conn::http1, service::service_fn};
use hyper_util::rt::{TokioIo, TokioTimer};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::Semaphore,
};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::{
    handshake::server::{Request, create_response},
    http::{Response, StatusCode},
};
use tokio_util::sync::CancellationToken;

struct Room {
    name: String,
    host_auth: Policy,
    client_auth: Policy,
    routes: Mutex<Routes>,
}
#[derive(Default)]
struct Routes {
    host: Option<Sender>,
    clients: HashMap<u64, Sender>,
}
struct Registration {
    room: Arc<Room>,
    role: Role,
    id: u64,
    sender: Sender,
}
impl Drop for Registration {
    fn drop(&mut self) {
        self.sender.stop.cancel();
        let mut routes = self.room.routes.lock().unwrap();
        match self.role {
            Role::Host => {
                routes.host = None;
                for (_, peer) in routes.clients.drain() {
                    peer.stop.cancel();
                }
            }
            Role::Client => {
                routes.clients.remove(&self.id);
            }
        }
    }
}

fn register(room: Arc<Room>, role: Role, sender: Sender, limit: usize) -> Result<Registration> {
    let id = {
        let mut routes = room.routes.lock().unwrap();
        match role {
            Role::Host => {
                ensure!(routes.host.is_none(), "Host already online");
                routes.host = Some(sender.clone());
                0
            }
            Role::Client => {
                ensure!(routes.host.is_some(), "Host offline");
                ensure!(routes.clients.len() < limit, "Room full");
                let mut id = rand::random::<u64>();
                while id == 0 || routes.clients.contains_key(&id) {
                    id = rand::random();
                }
                routes.clients.insert(id, sender.clone());
                id
            }
        }
    };
    Ok(Registration {
        room,
        role,
        id,
        sender,
    })
}

/// HTTP upgrade credentials are checked before registering a tunnel. Tokens
/// never appear in a URL, a response body, or a diagnostic log.
fn authenticate<B>(
    request: &hyper::Request<B>,
    rooms: &HashMap<String, Arc<Room>>,
) -> Option<(Arc<Room>, Role, [u8; 32])> {
    if request.uri().path() != PATH
        || request.uri().query().is_some()
        || request.headers().contains_key("origin")
    {
        return None;
    }
    let header = |key| request.headers().get(key)?.to_str().ok();
    if header("sec-websocket-protocol")? != PROTOCOL {
        return None;
    }
    let role = match header("x-removent-role")? {
        "host" => Role::Host,
        "client" => Role::Client,
        _ => return None,
    };
    let room = rooms.get(header("x-removent-room")?)?.clone();
    let token = match header("authorization") {
        Some(value) => value.strip_prefix("Bearer ")?,
        None => "",
    };
    let key = decode_secret(header("x-removent-key")?).ok()?;
    let policy = match role {
        Role::Host => &room.host_auth,
        Role::Client => &room.client_auth,
    };
    policy.authorize(token, &key).ok()?;
    Some((room, role, key))
}

struct Pending(Arc<AtomicUsize>);
impl Drop for Pending {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

struct Upgrade {
    room: Arc<Room>,
    role: Role,
    socket: hyper::upgrade::OnUpgrade,
    key: [u8; 32],
}

fn response(status: StatusCode, body: &'static [u8]) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("connection", "close")
        .body(Full::new(Bytes::from_static(body)))
        .unwrap()
}

async fn connection(
    socket: TcpStream,
    rooms: Arc<HashMap<String, Arc<Room>>>,
    config: &ServerConfig,
    pending: Arc<AtomicUsize>,
) -> Result<()> {
    socket.set_nodelay(true)?;
    let upgrade: Arc<Mutex<Option<Upgrade>>> = Arc::new(Mutex::new(None));
    let offered = upgrade.clone();
    let service = service_fn(move |mut request: hyper::Request<Incoming>| {
        let rooms = rooms.clone();
        let offered = offered.clone();
        async move {
            // Cloudflare's port-readiness probes are real HTTP requests, not
            // WebSocket upgrades. Always answer them with a complete response.
            let response =
                if request.method() == "GET" && matches!(request.uri().path(), "/" | "/healthz") {
                    response(StatusCode::OK, b"ok")
                } else if let Some((room, role, key)) = authenticate(&request, &rooms) {
                    let socket = hyper::upgrade::on(&mut request);
                    let request: Request = request.map(|_| ());
                    match create_response(&request) {
                        Ok(mut response) => {
                            response
                                .headers_mut()
                                .insert("sec-websocket-protocol", PROTOCOL.parse().unwrap());
                            *offered.lock().unwrap() = Some(Upgrade {
                                room,
                                role,
                                socket,
                                key,
                            });
                            response.map(|_| Full::new(Bytes::new()))
                        }
                        Err(_) => response(StatusCode::BAD_REQUEST, b"WebSocket upgrade required"),
                    }
                } else {
                    response(StatusCode::UNAUTHORIZED, b"Unauthorized")
                };
            Ok::<_, std::convert::Infallible>(response)
        }
    });
    tokio::time::timeout(
        crate::AUTH_TIMEOUT,
        http1::Builder::new()
            .max_buf_size(16 * 1024)
            .timer(TokioTimer::new())
            .header_read_timeout(crate::AUTH_TIMEOUT)
            .serve_connection(TokioIo::new(socket), service)
            .with_upgrades(),
    )
    .await??;
    let Some(Upgrade {
        room,
        role,
        socket,
        key,
    }) = upgrade.lock().unwrap().take()
    else {
        return Ok(());
    };
    let socket = tokio::time::timeout(crate::AUTH_TIMEOUT, socket).await??;
    let mut socket = tokio_tungstenite::WebSocketStream::from_raw_socket(
        TokioIo::new(socket),
        tokio_tungstenite::tungstenite::protocol::Role::Server,
        Some(limits()),
    )
    .await;
    tokio::time::timeout(crate::AUTH_TIMEOUT, async {
        let nonce: [u8; 32] = rand::random();
        socket.send(Message::Binary(nonce.to_vec().into())).await?;
        let Message::Binary(proof) = socket
            .next()
            .await
            .ok_or_else(|| anyhow::anyhow!("Missing proof"))??
        else {
            anyhow::bail!("Invalid proof");
        };
        auth::verify(
            &key,
            &auth::challenge_message(&room.name, role, &nonce),
            &proof,
        )
    })
    .await??;
    let mut tunnel = Tunnel::start(socket, 0);
    let pending_guard = if role == Role::Client {
        pending.fetch_add(1, Ordering::SeqCst);
        Some(Pending(pending))
    } else {
        None
    };
    // A controller may wake an empty container before the daemon's next retry.
    // Wait only after authentication, within the global connection bound.
    if role == Role::Client {
        tokio::time::timeout(Duration::from_secs(40), async {
            loop {
                if room.routes.lock().unwrap().host.is_some() {
                    return Ok::<_, anyhow::Error>(());
                }
                tokio::select! {
                    _ = tunnel.sender.stop.cancelled() => anyhow::bail!("Controller disconnected"),
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {},
                }
            }
        })
        .await??;
    }
    let reg = register(
        room,
        role,
        tunnel.sender.clone(),
        config.max_clients_per_room,
    )?;
    drop(pending_guard);
    tunnel.route = reg.id;
    tunnel
        .sender
        .raw(Bytes::copy_from_slice(&reg.id.to_be_bytes()))
        .await?;
    let mut budget = Budget::new(config.max_bytes_per_second);
    loop {
        let packet = tunnel.receive().await?;
        if !budget.allow(packet.len()) {
            continue;
        }
        let Some((id, _)) = parse_packet(&packet) else {
            continue;
        };
        let target = {
            let routes = reg.room.routes.lock().unwrap();
            match reg.role {
                Role::Client => {
                    ensure!(id == reg.id, "Invalid route");
                    routes.host.clone()
                }
                Role::Host => routes.clients.get(&id).cloned(),
            }
        };
        if let Some(target) = target {
            let _ = target.raw(packet).await;
        }
    }
}

/// Plain HTTP/WebSocket inside the container. Public connections use WSS at the
/// Cloudflare edge (or a TLS reverse proxy on a VPS).
pub async fn serve(
    listener: TcpListener,
    config: ServerConfig,
    stop: CancellationToken,
    idle: Option<Duration>,
) -> Result<()> {
    config.validate()?;
    let rooms = Arc::new(
        config
            .rooms
            .iter()
            .map(|r| {
                Ok((
                    r.name.clone(),
                    Arc::new(Room {
                        name: r.name.clone(),
                        host_auth: Policy::new(&r.host_token_sha256, &r.host_public_keys)?,
                        client_auth: Policy::new(&r.client_token_sha256, &r.client_public_keys)?,
                        routes: Mutex::new(Routes::default()),
                    }),
                ))
            })
            .collect::<Result<HashMap<_, _>>>()?,
    );
    let permits = Arc::new(Semaphore::new(config.max_connections));
    let pending = Arc::new(AtomicUsize::new(0));
    let mut tasks = tokio::task::JoinSet::new();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let mut last_controller = Instant::now();
    loop {
        tokio::select! {
            _ = stop.cancelled() => break,
            _ = tasks.join_next(), if !tasks.is_empty() => {},
            _ = tick.tick(), if idle.is_some() => {
                let controllers = pending.load(Ordering::SeqCst) + rooms.values().map(|r| r.routes.lock().unwrap().clients.len()).sum::<usize>();
                if controllers > 0 { last_controller = Instant::now(); }
                else if last_controller.elapsed() >= idle.unwrap() { break; }
            }
            incoming = listener.accept() => {
                let (socket, peer) = incoming?;
                if !config.allows_ip(peer.ip()) { drop(socket); continue; }
                let Ok(permit) = permits.clone().try_acquire_owned() else { drop(socket); continue; };
                let rooms = rooms.clone();
                let config = config.clone();
                let pending = pending.clone();
                tasks.spawn(async move {
                    let _permit = permit;
                    // Errors may originate in HTTP headers: do not log their payload.
                    let _ = connection(socket, rooms, &config, pending).await;
                });
            }
        }
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    Ok(())
}
