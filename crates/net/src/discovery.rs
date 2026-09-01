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

const ENTRY_TTL: Duration = Duration::from_secs(45);

pub struct Advertiser {
    daemon: ServiceDaemon,
    fullname: Mutex<Option<String>>,
    /// Parameters needed for re-registration (mdns-sd cannot change TXT records in place, only unregister+register).
    host: String,
    name: String,
    short_fp: String,
    port: u16,
    caps: Caps,
}

impl Advertiser {
    pub fn start(name: &str, short_fp: &str, port: u16, caps: Caps) -> Result<Self> {
        let daemon = ServiceDaemon::new().map_err(|e| NetError::Discovery(e.to_string()))?;
        let host = format!("removent-{}", short_fp);
        let svc = Self::build_info(host.clone(), name, short_fp, port, caps, false)?;
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
        })
    }

    fn build_info(
        host: String,
        name: &str,
        short_fp: &str,
        port: u16,
        caps: Caps,
        busy: bool,
    ) -> Result<ServiceInfo> {
        let instance = format!("{name}.{short_fp}");
        let props: HashMap<String, String> = [
            ("v".to_string(), "1".to_string()),
            ("name".to_string(), name.to_string()),
            ("fp".to_string(), short_fp.to_string()),
            ("cap".to_string(), cap_string(caps)),
            ("busy".to_string(), if busy { "1" } else { "0" }.to_string()),
        ]
        .into_iter()
        .collect();
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
        // mdns-sd cannot change TXT records in place: unregister first, then re-register the same instance name with the updated busy bit.
        let fullname = self.fullname.lock().unwrap().clone();
        let Some(fullname) = fullname else {
            return Err(NetError::Discovery("advertiser not running".into()));
        };
        let _ = self.daemon.unregister(&fullname);
        let svc = Self::build_info(
            self.host.clone(),
            &self.name,
            &self.short_fp,
            self.port,
            self.caps,
            busy,
        )?;
        self.daemon
            .register(svc)
            .map_err(|e| NetError::Discovery(e.to_string()))?;
        Ok(())
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

/// Browser: aggregates mDNS events and pushes a deduplicated device table to subscribers.
pub struct DiscoveryBrowser {
    _daemon: Arc<ServiceDaemon>,
    table_rx: watch::Receiver<Arc<HashMap<String, DeviceEntry>>>,
}

impl Clone for DiscoveryBrowser {
    fn clone(&self) -> Self {
        Self {
            _daemon: Arc::clone(&self._daemon),
            table_rx: self.table_rx.clone(),
        }
    }
}

impl DiscoveryBrowser {
    pub fn start() -> Result<Self> {
        let daemon =
            Arc::new(ServiceDaemon::new().map_err(|e| NetError::Discovery(e.to_string()))?);
        let receiver = daemon
            .browse(MDNS_SERVICE)
            .map_err(|e| NetError::Discovery(e.to_string()))?;

        let (table_tx, table_rx) = watch::channel(Arc::new(HashMap::new()));

        std::thread::Builder::new()
            .name("mdns-browser".into())
            .spawn(move || {
                let mut table: HashMap<String, DeviceEntry> = HashMap::new();
                loop {
                    // Periodically evict expired entries.
                    match recv_timeout(&receiver, Duration::from_secs(5)) {
                        Some(event) => match event {
                            ServiceEvent::ServiceResolved(info) => {
                                let props = info.get_properties();
                                let short_fp = props
                                    .get_property_val_str("fp")
                                    .unwrap_or_default()
                                    .to_string();
                                let name = props
                                    .get_property_val_str("name")
                                    .unwrap_or_else(|| info.get_fullname())
                                    .to_string();
                                let busy = props.get_property_val_str("busy") == Some("1");
                                let cap = parse_cap_string(
                                    props.get_property_val_str("cap").unwrap_or(""),
                                );
                                let addr: Option<std::net::SocketAddr> =
                                    pick_addr(info.get_addresses(), info.get_port());
                                let entry = DeviceEntry {
                                    instance: info.get_fullname().to_string(),
                                    name,
                                    short_fp,
                                    addr,
                                    caps: cap,
                                    busy,
                                    seen_at: Instant::now(),
                                };
                                table.insert(entry.instance.clone(), entry);
                            }
                            ServiceEvent::ServiceRemoved(_, fullname) => {
                                table.remove(&fullname);
                            }
                            _ => {}
                        },
                        None => {
                            table.retain(|_, e| !e.is_stale(ENTRY_TTL));
                        }
                    }
                    let _ = table_tx.send(Arc::new(table.clone()));
                }
            })?;

        Ok(Self {
            _daemon: daemon,
            table_rx,
        })
    }

    pub fn subscribe_table(&self) -> watch::Receiver<Arc<HashMap<String, DeviceEntry>>> {
        self.table_rx.clone()
    }
}

fn recv_timeout(
    receiver: &mdns_sd::Receiver<ServiceEvent>,
    timeout: Duration,
) -> Option<ServiceEvent> {
    use flume::TryRecvError;
    let deadline = Instant::now() + timeout;
    loop {
        match receiver.try_recv() {
            Ok(ev) => return Some(ev),
            Err(TryRecvError::Empty) => {
                if Instant::now() >= deadline {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(TryRecvError::Disconnected) => return None,
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_string_roundtrip() {
        let caps = Caps {
            video: true,
            audio: true,
            input: false,
            clipboard: true,
            file: false,
        };
        assert_eq!(cap_string(caps), "video,audio,clip");
        assert_eq!(parse_cap_string(&cap_string(caps)), caps);
        assert_eq!(parse_cap_string(""), Caps::none());
    }

    #[test]
    fn pick_addr_prefers_v4_and_keeps_v6_fallback() {
        let mut only_v6 = std::collections::HashSet::new();
        only_v6.insert("fe80::1".parse::<IpAddr>().unwrap());
        let addr = pick_addr(&only_v6, 48688).expect("v6 fallback");
        assert!(addr.is_ipv6());
        assert_eq!(addr.port(), 48688);

        let mut mixed = only_v6.clone();
        mixed.insert("192.168.1.2".parse::<IpAddr>().unwrap());
        let addr = pick_addr(&mixed, 48688).expect("v4 preferred");
        assert_eq!(addr.ip(), "192.168.1.2".parse::<IpAddr>().unwrap());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn advertise_and_browse_loopback() {
        let adv = Advertiser::start("TestMac", "deadbeef00112233", 48699, Caps::all()).unwrap();
        let browser = DiscoveryBrowser::start().unwrap();
        let mut rx = browser.subscribe_table();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut found = None;
        while Instant::now() < deadline {
            if tokio::time::timeout(Duration::from_millis(500), rx.changed())
                .await
                .is_err()
            {
                continue;
            }
            let table = rx.borrow_and_update().clone();
            if let Some(e) = table.values().find(|e| e.short_fp == "deadbeef00112233") {
                found = Some(e.clone());
                break;
            }
        }
        drop(adv);
        let entry = found.expect("should discover own advertiser on loopback network");
        assert_eq!(entry.name, "TestMac");
        assert!(entry.caps.video && entry.caps.clipboard);
    }
}
