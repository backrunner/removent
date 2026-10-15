//! Machine/protocol reservations shared by otherwise independent viewers.

use super::*;
use removent_client::connection::ConnectionAddress;

fn same_machine_host(a: &ConnectionAddress, b: &ConnectionAddress) -> bool {
    if a.same_host(&b.host) {
        return true;
    }
    match (
        a.host.parse::<std::net::IpAddr>(),
        b.host.parse::<std::net::IpAddr>(),
    ) {
        (Ok(a), Ok(b)) => {
            a.to_canonical() == b.to_canonical() || a.is_loopback() && b.is_loopback()
        }
        _ => false,
    }
}

pub(super) struct ClientSlot {
    pub generation: usize,
    pub target: ClientTarget,
    pub task: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl ClientSlot {
    pub fn is_live(&self) -> bool {
        self.task
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|task| !task.is_finished())
    }
}

/// Ports and credentials do not identify a machine. Relay rooms, however,
/// identify different machines behind the same relay server.
#[derive(Clone)]
pub(super) struct ClientTarget {
    protocol: ConnectionProtocol,
    hosts: Vec<ConnectionAddress>,
    rooms: Vec<(String, String)>,
    pins: Vec<[u8; 32]>,
    invitation: Option<String>,
}

impl ClientTarget {
    pub fn new(request: &ConnectionRequest) -> Self {
        let mut target = Self {
            protocol: request.protocol,
            hosts: Vec::new(),
            rooms: Vec::new(),
            pins: Vec::new(),
            invitation: None,
        };
        if let Some(route) = &request.relay {
            target
                .rooms
                .push((route.endpoint.clone(), request.address.host.clone()));
            target.pins.extend(route.optional_host_pin());
        } else if let Some(code) = &request.pairing_code {
            target.invitation = Some(code.locator().into());
        } else {
            target.hosts.push(request.address.clone());
        }
        target
    }

    pub fn overlaps(&self, other: &Self) -> bool {
        self.protocol == other.protocol
            && (self
                .hosts
                .iter()
                .any(|a| other.hosts.iter().any(|b| same_machine_host(a, b)))
                || self.rooms.iter().any(|room| other.rooms.contains(room))
                || self.pins.iter().any(|pin| other.pins.contains(pin))
                || self.invitation.is_some() && self.invitation == other.invitation)
    }
}

impl ClientAttempt {
    /// Enrich the reservation after DNS, invitation resolution or native TLS.
    /// Check and publication are atomic even for simultaneous alias connects.
    fn claim_target(&self, update: impl FnOnce(&mut ClientTarget)) -> Result<()> {
        let Some(sessions) = self.sessions.upgrade() else {
            return Ok(()); // Standalone protocol bridge tests have no registry.
        };
        let mut sessions = sessions.lock().unwrap();
        sessions.retain(ClientSlot::is_live);
        let index = sessions
            .iter()
            .position(|s| s.generation == self.generation)
            .ok_or_else(|| anyhow::anyhow!("Connection cancelled"))?;
        let mut target = sessions[index].target.clone();
        update(&mut target);
        anyhow::ensure!(
            !sessions
                .iter()
                .enumerate()
                .any(|(i, slot)| i != index && slot.target.overlaps(&target)),
            t!("err.session_in_progress").to_string()
        );
        sessions[index].target = target;
        Ok(())
    }

    pub(super) fn claim_addresses(&self, addresses: &[SocketAddr]) -> Result<()> {
        self.claim_target(|target| {
            for address in addresses {
                let address = ConnectionAddress::from_socket(*address);
                if !target
                    .hosts
                    .iter()
                    .any(|host| host.same_host(&address.host))
                {
                    target.hosts.push(address);
                }
            }
        })
    }

    pub(super) fn claim_fingerprint(&self, pin: [u8; 32]) -> Result<()> {
        self.claim_target(|target| {
            if !target.pins.contains(&pin) {
                target.pins.push(pin);
            }
        })
    }

    pub(super) fn claim_relay_room(&self, endpoint: &str, room: &str) -> Result<()> {
        self.claim_target(|target| {
            let route = (endpoint.to_owned(), room.to_owned());
            if !target.rooms.contains(&route) {
                target.rooms.push(route);
            }
        })
    }
}

#[cfg(test)]
impl Engine {
    pub(crate) fn publish_test_frames(
        &self,
        commands: tokio::sync::mpsc::Sender<ControlMsg>,
    ) -> removent_core::latest::Sender<VideoFrame> {
        ClientAttempt {
            generation: self.client_generation(),
            started: std::time::Instant::now(),
            recovery: self.client_recovery(),
            channels: self.client_channels.clone(),
            events: self.events_tx.clone(),
            sessions: Arc::downgrade(&self.client_sessions),
        }
        .publish(commands, "test".into())
        .unwrap()
    }
}

#[cfg(test)]
mod tests;
