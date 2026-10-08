//! mDNS discovery (protocol.md §3).

use crate::error::{NetError, Result};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use removent_proto::{Caps, MDNS_SERVICE};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::watch;

#[derive(Debug, Clone, PartialEq)]
pub struct DeviceEntry {
    pub pairing: Option<removent_core::pairing_invitation::Advertisement>,
    pub instance: String,
    pub name: String,
    pub short_fp: String,
    pub addr: Option<std::net::SocketAddr>,
    pub caps: Caps,
    pub busy: bool,
    /// When this entry was last resolved, used for TTL graying-out.
    pub seen_at: Instant,
}

impl DeviceEntry {
    pub fn is_stale(&self, ttl: Duration) -> bool {
        self.seen_at.elapsed() > ttl
    }
}

/// ServiceDaemon handles do not stop their worker when dropped.
struct OwnedDaemon(ServiceDaemon);

impl std::ops::Deref for OwnedDaemon {
    type Target = ServiceDaemon;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for OwnedDaemon {
    fn drop(&mut self) {
        let _ = self.0.shutdown();
    }
}

pub struct Advertiser {
    daemon: OwnedDaemon,
    fullname: Mutex<Option<String>>,
    /// Parameters needed to re-announce updated TXT records.
    host: String,
    name: String,
    short_fp: String,
    port: u16,
    caps: Caps,
    state: Mutex<(
        bool,
        Option<removent_core::pairing_invitation::Advertisement>,
    )>,
}

impl Advertiser {
    pub fn start(name: &str, short_fp: &str, port: u16, caps: Caps) -> Result<Self> {
        let daemon =
            OwnedDaemon(ServiceDaemon::new().map_err(|e| NetError::Discovery(e.to_string()))?);
        // Native Bonjour clients (including an iOS Simulator) on this computer
        // must also receive the advertisement emitted by the Rust daemon.
        daemon
            .set_multicast_loop_v4(true)
            .map_err(|e| NetError::Discovery(e.to_string()))?;
        daemon
            .set_multicast_loop_v6(true)
            .map_err(|e| NetError::Discovery(e.to_string()))?;
        Self::with_daemon(daemon, name, short_fp, port, caps)
    }

    fn with_daemon(
        daemon: OwnedDaemon,
        name: &str,
        short_fp: &str,
        port: u16,
        caps: Caps,
    ) -> Result<Self> {
        let host = format!("removent-{}", short_fp);
        let svc = Self::build_info(host.clone(), name, short_fp, port, caps, false, None)?;
        let fullname = svc.get_fullname().to_string();
        daemon
            .register(svc)
            .map_err(|e| NetError::Discovery(e.to_string()))?;
        Ok(Self {
            daemon,
            fullname: Mutex::new(Some(fullname)),
            host,
            name: name.to_string(),
            short_fp: short_fp.to_string(),
            port,
            caps,
            state: Mutex::new((false, None)),
        })
    }

    fn build_info(
        host: String,
        name: &str,
        short_fp: &str,
        port: u16,
        caps: Caps,
        busy: bool,
        pairing: Option<&removent_core::pairing_invitation::Advertisement>,
    ) -> Result<ServiceInfo> {
        // DNS-SD instance names occupy one DNS label. mdns-sd serializes dots
        // as label separators, which native Bonjour cannot browse.
        let mut label = name.replace('.', " ");
        let available = 63usize.saturating_sub(short_fp.len() + 3);
        let mut end = label.len().min(available);
        while !label.is_char_boundary(end) {
            end -= 1;
        }
        label.truncate(end);
        let instance = format!("{label} - {short_fp}");
        let mut props: HashMap<String, String> = [
            ("v".to_string(), "1".to_string()),
            ("name".to_string(), name.to_string()),
            ("fp".to_string(), short_fp.to_string()),
            ("cap".to_string(), cap_string(caps)),
            ("busy".to_string(), if busy { "1" } else { "0" }.to_string()),
        ]
        .into_iter()
        .collect();
        if let Some(ad) = pairing.filter(|ad| ad.valid()) {
            props.insert("pair".into(), ad.locator.clone());
            props.insert("pair-exp".into(), ad.expires_at_unix.to_string());
        }
        ServiceInfo::new(
            MDNS_SERVICE,
            &instance,
            &format!("{host}.local."),
            "",
            port,
            props,
        )
        .map_err(|e| NetError::Discovery(e.to_string()))
        .map(|info| info.enable_addr_auto())
    }

    /// Update the busy bit when a session starts/ends.
    pub fn set_busy(&self, busy: bool) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        state.0 = busy;
        self.announce(&state)
    }
    pub fn set_pairing(
        &self,
        pairing: Option<removent_core::pairing_invitation::Advertisement>,
    ) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        state.1 = pairing;
        self.announce(&state)
    }
    fn announce(
        &self,
        state: &(
            bool,
            Option<removent_core::pairing_invitation::Advertisement>,
        ),
    ) -> Result<()> {
        // Registering the same fullname updates its TXT records. Unregistering
        // first sends goodbyes (including a delayed retry), which can remove a
        // still-running host from peer caches after it has been re-announced.
        let svc = Self::build_info(
            self.host.clone(),
            &self.name,
            &self.short_fp,
            self.port,
            self.caps,
            state.0,
            state.1.as_ref(),
        )?;
        self.daemon
            .register(svc)
            .map_err(|e| NetError::Discovery(e.to_string()))?;
        Ok(())
    }
}

impl Drop for Advertiser {
    fn drop(&mut self) {
        if let Some(fullname) = self.fullname.get_mut().unwrap().take() {
            // Queue the goodbye before OwnedDaemon queues shutdown.
            let _ = self.daemon.unregister(&fullname);
        }
    }
}

pub fn cap_string(caps: Caps) -> String {
    let mut parts = Vec::new();
    if caps.video {
        parts.push("video");
    }
    if caps.audio {
        parts.push("audio");
    }
    if caps.input {
        parts.push("input");
    }
    if caps.clipboard {
        parts.push("clip");
    }
    if caps.file {
        parts.push("file");
    }
    parts.join(",")
}

pub fn parse_cap_string(s: &str) -> Caps {
    Caps {
        video: s.contains("video"),
        audio: s.contains("audio"),
        input: s.contains("input"),
        clipboard: s.contains("clip"),
        file: s.contains("file"),
    }
}

/// DNS-SD service types advertised by compatible remote desktop servers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryProtocol {
    Removent,
    Vnc,
    Rdp,
}

impl DiscoveryProtocol {
    pub fn service_types(self) -> &'static [&'static str] {
        match self {
            Self::Removent => &[MDNS_SERVICE],
            Self::Vnc => &["_rfb._tcp.local."],
            Self::Rdp => &["_rdp._tcp.local.", "_ms-wbt-server._tcp.local."],
        }
    }
}

/// Browser for one protocol. Dropping the last owner stops all its queries.
#[derive(Clone)]
pub struct DiscoveryBrowser {
    _daemon: Arc<OwnedDaemon>,
    table_rx: watch::Receiver<Arc<HashMap<String, DeviceEntry>>>,
}

impl DiscoveryBrowser {
    /// Native-only discovery, also used by the CLI's explicit target lookup.
    pub fn start() -> Result<Self> {
        Self::start_for(DiscoveryProtocol::Removent)
    }

    pub fn start_for(protocol: DiscoveryProtocol) -> Result<Self> {
        let daemon =
            OwnedDaemon(ServiceDaemon::new().map_err(|e| NetError::Discovery(e.to_string()))?);
        Self::with_daemon(daemon, protocol)
    }

    fn with_daemon(daemon: OwnedDaemon, protocol: DiscoveryProtocol) -> Result<Self> {
        let daemon = Arc::new(daemon);
        let (table_tx, table_rx) = watch::channel(Arc::new(HashMap::new()));
        let table = Arc::new(Mutex::new(HashMap::<String, DeviceEntry>::new()));
        for service_type in protocol.service_types() {
            let receiver = daemon
                .browse(service_type)
                .map_err(|e| NetError::Discovery(e.to_string()))?;
            let table = table.clone();
            let table_tx = table_tx.clone();
            std::thread::Builder::new()
                .name(format!("mdns-{protocol:?}"))
                .spawn(move || {
                    while let Ok(event) = receiver.recv() {
                        // mdns-sd owns TTL refresh/expiry and goodbye handling.
                        let mut table = table.lock().unwrap();
                        match event {
                            ServiceEvent::ServiceResolved(info) => {
                                if let Some(entry) = resolved_entry(protocol, &info) {
                                    table.insert(entry.instance.clone(), entry);
                                } else {
                                    table.remove(info.get_fullname());
                                }
                            }
                            ServiceEvent::ServiceRemoved(_, fullname) => {
                                table.remove(&fullname);
                            }
                            ServiceEvent::SearchStarted(interfaces) => {
                                tracing::debug!(?protocol, %interfaces, "mDNS browse started");
                                continue;
                            }
                            _ => continue,
                        }
                        if table_tx.send(Arc::new(table.clone())).is_err() {
                            break;
                        }
                    }
                })?;
        }
        Ok(Self {
            _daemon: daemon,
            table_rx,
        })
    }

    pub fn subscribe_table(&self) -> watch::Receiver<Arc<HashMap<String, DeviceEntry>>> {
        self.table_rx.clone()
    }
}

fn resolved_entry(protocol: DiscoveryProtocol, info: &ServiceInfo) -> Option<DeviceEntry> {
    let props = info.get_properties();
    let native = protocol == DiscoveryProtocol::Removent;
    let short_fp = if native {
        props.get_property_val_str("fp").unwrap_or_default()
    } else {
        ""
    };
    // Never treat an arbitrary compatibility TXT record as a pairing identity.
    if info.get_port() == 0
        || (native && (short_fp.len() != 16 || !short_fp.bytes().all(|b| b.is_ascii_hexdigit())))
    {
        return None;
    }
    let instance_name = info
        .get_fullname()
        .strip_suffix(info.get_type())
        .unwrap_or(info.get_fullname())
        .trim_end_matches('.');
    let name = if native {
        props.get_property_val_str("name").unwrap_or(instance_name)
    } else {
        instance_name
    };
    let pairing = native
        .then(|| removent_core::pairing_invitation::Advertisement {
            locator: props
                .get_property_val_str("pair")
                .unwrap_or_default()
                .into(),
            expires_at_unix: props
                .get_property_val_str("pair-exp")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
        })
        .filter(|ad| ad.valid());
    Some(DeviceEntry {
        pairing,
        instance: info.get_fullname().to_string(),
        name: name.to_string(),
        short_fp: short_fp.to_ascii_lowercase(),
        addr: pick_addr(info.get_addresses(), info.get_port()),
        caps: if native {
            parse_cap_string(props.get_property_val_str("cap").unwrap_or(""))
        } else {
            Caps::none()
        },
        busy: native && props.get_property_val_str("busy") == Some("1"),
        seen_at: Instant::now(),
    })
}

/// Prefer IPv4; keep the first IPv6 when no v4 exists (v6 is no longer dropped unconditionally).
fn pick_addr(addrs: &std::collections::HashSet<IpAddr>, port: u16) -> Option<std::net::SocketAddr> {
    let mut first_v6 = None;
    for ip in addrs {
        match ip {
            IpAddr::V4(_) => return Some(std::net::SocketAddr::new(*ip, port)),
            IpAddr::V6(_) => {
                if first_v6.is_none() {
                    first_v6 = Some(*ip);
                }
            }
        }
    }
    first_v6.map(|ip| std::net::SocketAddr::new(ip, port))
}

/// Resolve the public locator only. The private PIN is verified by RVP PAKE.
pub async fn resolve_pairing_locator(locator: &str) -> Result<std::net::SocketAddr> {
    let browser = DiscoveryBrowser::start()?;
    let mut rx = browser.subscribe_table();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let destinations = pairing_destinations(&rx.borrow_and_update(), locator);
            if destinations.len() > 1 {
                return Err(NetError::Discovery(
                    "Ambiguous pairing code; generate a new code".into(),
                ));
            }
            if !destinations.is_empty() {
                tokio::time::sleep(Duration::from_millis(300)).await;
                let current = pairing_destinations(&rx.borrow_and_update(), locator);
                if current.len() > 1 {
                    return Err(NetError::Discovery(
                        "Ambiguous pairing code; generate a new code".into(),
                    ));
                }
                if let Some(addr) = current.values().next() {
                    return Ok(*addr);
                }
            }
            rx.changed()
                .await
                .map_err(|_| NetError::Discovery("Discovery stopped".into()))?;
        }
    })
    .await
    .map_err(|_| NetError::Discovery("Pairing code not found on this network or expired".into()))?
}

// Bonjour may resolve the same host with interface/conflict variants. The
// locator must identify one device identity, rather than one service record.
fn pairing_destinations(
    table: &HashMap<String, DeviceEntry>,
    locator: &str,
) -> std::collections::BTreeMap<String, std::net::SocketAddr> {
    table
        .values()
        .filter(|entry| {
            entry
                .pairing
                .as_ref()
                .is_some_and(|ad| ad.valid() && ad.locator == locator)
        })
        .filter_map(|entry| entry.addr.map(|addr| (entry.short_fp.clone(), addr)))
        .collect()
}

#[cfg(test)]
mod tests;
