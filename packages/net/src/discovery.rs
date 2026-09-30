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
}

impl Advertiser {
    pub fn start(name: &str, short_fp: &str, port: u16, caps: Caps) -> Result<Self> {
        let daemon =
            OwnedDaemon(ServiceDaemon::new().map_err(|e| NetError::Discovery(e.to_string()))?);
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
        // Registering the same fullname updates its TXT records. Unregistering
        // first sends goodbyes (including a delayed retry), which can remove a
        // still-running host from peer caches after it has been re-announced.
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
    Some(DeviceEntry {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compatibility_records_do_not_require_or_inherit_native_identity() {
        for protocol in [DiscoveryProtocol::Vnc, DiscoveryProtocol::Rdp] {
            for service_type in protocol.service_types() {
                let info = ServiceInfo::new(
                    service_type,
                    "Office PC",
                    "office.local.",
                    "192.168.1.8",
                    3391,
                    &[
                        ("fp", "spoofed"),
                        ("busy", "1"),
                        ("cap", "file"),
                        ("name", "spoofed"),
                    ][..],
                )
                .unwrap();
                let entry = resolved_entry(protocol, &info).unwrap();
                assert_eq!(entry.name, "Office PC");
                assert_eq!(entry.addr.unwrap().port(), 3391);
                assert!(entry.short_fp.is_empty());
                assert!(!entry.busy);
                assert_eq!(entry.caps, Caps::none());
            }
        }
        let info = ServiceInfo::new(
            MDNS_SERVICE,
            "No identity",
            "office.local.",
            "127.0.0.1",
            48688,
            None,
        )
        .unwrap();
        assert!(resolved_entry(DiscoveryProtocol::Removent, &info).is_none());
        let spoofed = ServiceInfo::new(
            MDNS_SERVICE,
            "Invalid identity",
            "office.local.",
            "127.0.0.1",
            48688,
            &[("fp", "VNC:127.0.0.1:5900")][..],
        )
        .unwrap();
        assert!(resolved_entry(DiscoveryProtocol::Removent, &spoofed).is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compatibility_protocols_resolve_and_remove_over_multicast() {
        let native =
            DiscoveryBrowser::with_daemon(loopback_daemon(), DiscoveryProtocol::Removent).unwrap();
        for protocol in [DiscoveryProtocol::Vnc, DiscoveryProtocol::Rdp] {
            let browser = DiscoveryBrowser::with_daemon(loopback_daemon(), protocol).unwrap();
            let mut rx = browser.subscribe_table();
            let advertiser = loopback_daemon();
            for service_type in protocol.service_types() {
                let name = format!("Desktop-{:016x}", rand::random::<u64>());
                let info = ServiceInfo::new(
                    service_type,
                    &name,
                    "compat.local.",
                    "127.0.0.1",
                    3391,
                    None,
                )
                .unwrap();
                let fullname = info.get_fullname().to_string();
                advertiser.register(info).unwrap();
                tokio::time::timeout(Duration::from_secs(10), async {
                    loop {
                        if let Some(entry) = rx.borrow_and_update().get(&fullname) {
                            assert_eq!(entry.name, name);
                            assert_eq!(entry.addr, Some("127.0.0.1:3391".parse().unwrap()));
                            break;
                        }
                        rx.changed().await.unwrap();
                    }
                })
                .await
                .expect("compatibility service must resolve");
                assert!(native.subscribe_table().borrow().is_empty());
                advertiser.unregister(&fullname).unwrap();
                tokio::time::timeout(Duration::from_secs(5), async {
                    while rx.borrow_and_update().contains_key(&fullname) {
                        rx.changed().await.unwrap();
                    }
                })
                .await
                .expect("goodbye must remove compatibility service");
            }
            drop(browser);
            tokio::time::timeout(Duration::from_secs(2), async {
                while rx.changed().await.is_ok() {}
            })
            .await
            .expect("all protocol workers must stop on drop");
        }
    }

    #[tokio::test]
    async fn dropping_last_browser_closes_the_table_channel() {
        let browser = DiscoveryBrowser::start().unwrap();
        let mut table = browser.subscribe_table();
        let clone = browser.clone();
        drop(browser);
        assert!(table.has_changed().is_ok());
        drop(clone);
        tokio::time::timeout(Duration::from_secs(2), async {
            while table.changed().await.is_ok() {}
        })
        .await
        .expect("browser worker and publisher must stop on last owner drop");
    }

    #[tokio::test]
    async fn dropping_advertiser_stops_its_daemon() {
        let advertiser =
            Advertiser::start("DropTest", "0000000000000001", 48699, Caps::all()).unwrap();
        let daemon = advertiser.daemon.0.clone();
        drop(advertiser);
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match daemon.status() {
                    Ok(status) => {
                        // A status command queued behind Exit may never be
                        // processed; retry status() to observe disconnection.
                        if matches!(
                            tokio::time::timeout(Duration::from_millis(50), status.recv_async())
                                .await,
                            Ok(Ok(mdns_sd::DaemonStatus::Shutdown) | Err(_))
                        ) {
                            break;
                        }
                    }
                    Err(_) => break,
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("advertiser must not remain alive after host shutdown");
    }

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

    // Exercise actual multicast between independent daemons, but keep it on
    // loopback so LAN permissions, Wi-Fi isolation and VPN routes do not decide
    // whether the regression suite passes. mdns-sd disables loopback by default.
    fn loopback_daemon() -> OwnedDaemon {
        let daemon = OwnedDaemon(ServiceDaemon::new().unwrap());
        daemon.disable_interface(mdns_sd::IfKind::All).unwrap();
        daemon
            .enable_interface(mdns_sd::IfKind::LoopbackV4)
            .unwrap();
        daemon
    }

    async fn wait_for_entry(
        rx: &mut watch::Receiver<Arc<HashMap<String, DeviceEntry>>>,
        fp: &str,
        busy: bool,
        must_remain_present: bool,
    ) -> DeviceEntry {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                {
                    let table = rx.borrow_and_update();
                    let entry = table.values().find(|entry| entry.short_fp == fp);
                    if must_remain_present {
                        assert!(entry.is_some(), "TXT update must not remove the device");
                    }
                    if let Some(entry) = entry.filter(|entry| entry.busy == busy) {
                        return entry.clone();
                    }
                }
                rx.changed().await.expect("browser stopped unexpectedly");
            }
        })
        .await
        .expect("service must resolve over IPv4 loopback multicast within 10 seconds")
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn advertise_and_browse_loopback() {
        let fp = format!("{:016x}", rand::random::<u64>());
        let browser =
            DiscoveryBrowser::with_daemon(loopback_daemon(), DiscoveryProtocol::Removent).unwrap();
        let mut rx = browser.subscribe_table();
        let adv =
            Advertiser::with_daemon(loopback_daemon(), "TestMac", &fp, 48699, Caps::all()).unwrap();
        let entry = wait_for_entry(&mut rx, &fp, false, false).await;
        assert_eq!(entry.name, "TestMac");
        assert_eq!(entry.addr, Some("127.0.0.1:48699".parse().unwrap()));
        assert_eq!(entry.caps, Caps::all());

        for busy in [true, false, true, false] {
            adv.set_busy(busy).unwrap();
            wait_for_entry(&mut rx, &fp, busy, true).await;
        }
        // An update must never queue an unregister/goodbye, even if a watch
        // receiver coalesces an intermediate removal and reappearance.
        let metrics = adv
            .daemon
            .get_metrics()
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        assert_eq!(metrics.get("unregister").copied().unwrap_or(0), 0);

        drop(adv);
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if !rx.borrow_and_update().contains_key(&entry.instance) {
                    break;
                }
                rx.changed().await.expect("browser stopped before goodbye");
            }
        })
        .await
        .expect("goodbye must remove the stopped advertiser");
    }
}
