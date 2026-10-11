//! Shared desktop/mobile recovery. Offline time never spends the retry budget.

use std::{
    future::Future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{sync::watch, time::Instant};

mod network;
#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    Connecting,
    Connected,
    Offline,
    /// Only recovery following a system outage needs a visible overlay.
    Reconnecting {
        after_offline: bool,
    },
    Failed(String),
    Ended,
}

/// The rotating token stays live even when the supervising future is cancelled
/// by an OS network outage. Resumption always pins the authenticated host.
#[derive(Clone)]
pub struct NativeResume {
    pub token: Arc<Mutex<Option<[u8; 16]>>>,
    pub negotiated: removent_proto::NegotiateAck,
    pub fingerprint: [u8; 32],
}

impl NativeResume {
    pub async fn reconnect(
        &self,
        address: std::net::SocketAddr,
        identity: &removent_core::DeviceIdentity,
        mut config: crate::ClientConfig,
    ) -> anyhow::Result<crate::ClientSession> {
        let bind = if address.is_ipv6() {
            "[::]:0"
        } else {
            "0.0.0.0:0"
        }
        .parse()?;
        let (endpoint, _) = removent_net::make_client_endpoint(
            bind,
            identity,
            removent_net::PinState::new([self.fingerprint], false),
        )?;
        config.pairing_code = None;
        let token = *self.token.lock().unwrap();
        Ok(if let Some(token) = token {
            // quick_resume already falls back to full negotiation for an expired
            // token. No credential UI is allowed during background recovery.
            crate::quick_resume(
                endpoint,
                address,
                identity,
                config,
                token,
                self.negotiated.clone(),
            )
            .await?
        } else {
            crate::connect_session_with_confirmation(
                endpoint, address, identity, config, None, None, None, None,
            )
            .await?
        })
    }
}

#[derive(Clone)]
pub struct Recovery {
    state: watch::Sender<State>,
    connections: Arc<AtomicU64>,
    connected_at: Arc<Mutex<Option<Instant>>>,
    pub native: Arc<Mutex<Option<NativeResume>>>,
    /// An invitation must resolve only once; retry the authenticated destination.
    pub destination: Arc<Mutex<Option<crate::connection::ConnectionRequest>>>,
}

impl Default for Recovery {
    fn default() -> Self {
        Self {
            state: watch::channel(State::Connecting).0,
            connections: Arc::new(AtomicU64::new(0)),
            connected_at: Arc::new(Mutex::new(None)),
            native: Arc::new(Mutex::new(None)),
            destination: Arc::new(Mutex::new(None)),
        }
    }
}

impl Recovery {
    pub fn subscribe(&self) -> watch::Receiver<State> {
        self.state.subscribe()
    }
    pub fn state(&self) -> State {
        self.state.borrow().clone()
    }
    pub fn connected(&self) {
        *self.connected_at.lock().unwrap() = Some(Instant::now());
        self.connections.fetch_add(1, Ordering::SeqCst);
        self.state.send_replace(State::Connected);
    }
    pub fn has_connected(&self) -> bool {
        self.connections.load(Ordering::SeqCst) > 0
    }
    pub fn connection_epoch(&self) -> u64 {
        self.connections.load(Ordering::SeqCst)
    }
    pub fn finish(&self, error: Option<String>) {
        self.state
            .send_replace(error.map(State::Failed).unwrap_or(State::Ended));
    }

    /// run_once owns all transport resources. Dropping it on offline/cancel
    /// tears down the old sockets and relay bridge before any replacement starts.
    pub async fn run<F, Fut>(&self, run_once: F) -> anyhow::Result<()>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = anyhow::Result<()>>,
    {
        self.run_with_network(run_once, network::online).await
    }

    async fn run_with_network<F, Fut, N>(&self, mut run_once: F, online: N) -> anyhow::Result<()>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = anyhow::Result<()>>,
        N: Fn() -> bool,
    {
        const LIMIT: Duration = Duration::from_secs(45);
        const ATTEMPT_LIMIT: Duration = Duration::from_secs(12);
        const DELAYS: [u64; 5] = [0, 1, 2, 4, 8];
        let mut retries = 0;
        let mut deadline = None;
        let mut after_offline = false;
        let mut last_error = anyhow::anyhow!("Connection interrupted");
        loop {
            if !online() {
                self.state.send_replace(State::Offline);
                while !online() {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
                // A fresh online window, including when the first connection
                // was interrupted. Do not expire while the user is offline.
                after_offline = true;
                retries = 0;
                deadline = Some(Instant::now() + LIMIT);
            }
            if let Some(end) = deadline {
                if retries == DELAYS.len() || Instant::now() >= end {
                    return Err(last_error
                        .context("Automatic reconnect exhausted its retry limit or timeout"));
                }
                self.state
                    .send_replace(State::Reconnecting { after_offline });
                let delay = Duration::from_secs(DELAYS[retries]);
                tokio::select! {
                    biased;
                    _ = wait_offline(&online) => continue,
                    _ = tokio::time::sleep_until((Instant::now() + delay).min(end)) => {}
                }
                if !online() {
                    continue;
                }
                if Instant::now() >= end {
                    continue;
                }
                retries += 1;
            }
            let epoch = self.connections.load(Ordering::SeqCst);
            let mut changes = self.subscribe();
            let attempt_deadline = deadline.map(|end| end.min(Instant::now() + ATTEMPT_LIMIT));
            let result = {
                let future = run_once();
                tokio::pin!(future);
                loop {
                    // Ready clears the connect deadline, so a healthy session
                    // can run indefinitely. A later drop gets a new budget.
                    let ready = self.connections.load(Ordering::SeqCst) != epoch;
                    tokio::select! {
                        biased;
                        _ = wait_offline(&online) => break None,
                        result = &mut future => break Some(result),
                        _ = changes.changed() => {},
                        _ = async { tokio::time::sleep_until(attempt_deadline.unwrap()).await },
                            if attempt_deadline.is_some() && !ready => {
                            break Some(Err(anyhow::anyhow!("Reconnect attempt timed out")));
                        }
                    }
                }
            }; // Drop the transport before retrying.
            let Some(result) = result else {
                self.state.send_replace(State::Offline);
                after_offline = true;
                retries = 0;
                deadline = Some(Instant::now() + LIMIT);
                continue;
            };
            match result {
                Ok(()) => return Ok(()), // Explicit peer/local shutdown.
                Err(error) => {
                    last_error = error;
                    if !online() {
                        continue;
                    }
                    if !self.has_connected() && deadline.is_none() {
                        return Err(last_error);
                    }
                    // Rapid handshake/drop loops must not replenish the retry
                    // budget indefinitely. Reset only after a stable session.
                    let stable = self.connections.load(Ordering::SeqCst) != epoch
                        && self
                            .connected_at
                            .lock()
                            .unwrap()
                            .is_some_and(|at| at.elapsed() >= Duration::from_secs(10));
                    if deadline.is_none() || stable {
                        retries = 0;
                        after_offline = false;
                        deadline = Some(Instant::now() + LIMIT);
                    }
                }
            }
        }
    }
}

async fn wait_offline(online: &impl Fn() -> bool) {
    while online() {
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}
