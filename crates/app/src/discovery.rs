//! Live per-protocol discovery policy and stable UI identities.

use crate::engine::UiEvent;
use removent_client::connection::ConnectionProtocol;
use removent_core::DiscoverySettings;
use removent_net::{DeviceEntry, DiscoveryBrowser, DiscoveryProtocol, lan_scan::LanScanner};
use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::Duration;
use tokio::sync::watch;

pub fn enabled(settings: DiscoverySettings, protocol: ConnectionProtocol) -> bool {
    match protocol {
        ConnectionProtocol::Removent => settings.removent,
        ConnectionProtocol::Vnc => settings.vnc,
        ConnectionProtocol::Rdp => settings.rdp,
    }
}

pub async fn run(
    protocol: ConnectionProtocol,
    settings: watch::Receiver<DiscoverySettings>,
    my_fp: String,
    events: Sender<UiEvent>,
) {
    run_with_browser(
        protocol,
        settings,
        my_fp,
        events,
        DiscoveryBrowser::start_for,
        LanScanner::start,
    )
    .await;
}

async fn run_with_browser(
    protocol: ConnectionProtocol,
    mut settings: watch::Receiver<DiscoverySettings>,
    my_fp: String,
    events: Sender<UiEvent>,
    start: impl Fn(DiscoveryProtocol) -> removent_net::Result<DiscoveryBrowser>,
    start_scan: impl Fn(DiscoveryProtocol) -> Option<LanScanner>,
) {
    let browse_protocol = match protocol {
        ConnectionProtocol::Removent => DiscoveryProtocol::Removent,
        ConnectionProtocol::Vnc => DiscoveryProtocol::Vnc,
        ConnectionProtocol::Rdp => DiscoveryProtocol::Rdp,
    };
    loop {
        if !enabled(*settings.borrow_and_update(), protocol) {
            if settings.changed().await.is_err() {
                return;
            }
            continue;
        }
        let scanner = start_scan(browse_protocol);
        let mut scanned = scanner.as_ref().map(LanScanner::subscribe_table);
        let mut browser = None;
        let mut advertised = None;
        let mut retry_at = tokio::time::Instant::now();
        let mut previous = BTreeMap::new();
        loop {
            if browser.is_none() && tokio::time::Instant::now() >= retry_at {
                match start(browse_protocol) {
                    Ok(next) => {
                        advertised = Some(next.subscribe_table());
                        browser = Some(next);
                    }
                    Err(error) => {
                        tracing::warn!(?protocol, %error, "mDNS discovery failed");
                        let _ = events.send(UiEvent::Notice(
                            rust_i18n::t!(
                                "notice.discovery_unavailable",
                                err = format!("{} mDNS: {error}", protocol.short_label())
                            )
                            .to_string(),
                        ));
                        retry_at = tokio::time::Instant::now() + Duration::from_secs(5);
                    }
                }
            }
            let mut combined = HashMap::new();
            for source in [&mut scanned, &mut advertised].into_iter().flatten() {
                combined.extend(
                    source
                        .borrow_and_update()
                        .iter()
                        .map(|(id, entry)| (id.clone(), entry.clone())),
                );
            }
            let next = visible_devices(protocol, &my_fp, &combined);
            publish(protocol, &previous, &next, &events);
            previous = next;
            tokio::select! {
                result = settings.changed() => {
                    if result.is_err() || !enabled(*settings.borrow_and_update(), protocol) {
                        break;
                    }
                    // Watch can coalesce a quick off/on. Re-publish rows cleared
                    // immediately by the UI even if discovery stayed live.
                    publish(protocol, &BTreeMap::new(), &previous, &events);
                }
                result = changed(&mut advertised) => if result.is_err() {
                    advertised = None;
                    browser = None;
                    retry_at = tokio::time::Instant::now() + Duration::from_secs(5);
                },
                result = changed(&mut scanned) => if result.is_err() { scanned = None; },
                _ = tokio::time::sleep_until(retry_at), if browser.is_none() => {},
            }
        }
        // Stops multicast queries and all active TCP probes for this protocol.
        drop(scanner);
        drop(browser);
        publish(protocol, &previous, &BTreeMap::new(), &events);
        if settings.has_changed().is_err() {
            return;
        }
    }
}

type DiscoveryTable = watch::Receiver<Arc<HashMap<String, DeviceEntry>>>;

async fn changed(source: &mut Option<DiscoveryTable>) -> Result<(), watch::error::RecvError> {
    match source {
        Some(source) => source.changed().await,
        None => std::future::pending().await,
    }
}

type VisibleDevices = BTreeMap<String, (String, SocketAddr)>;

fn visible_devices(
    protocol: ConnectionProtocol,
    my_fp: &str,
    table: &HashMap<String, DeviceEntry>,
) -> VisibleDevices {
    let mut entries: BTreeMap<String, &DeviceEntry> = BTreeMap::new();
    for entry in table.values() {
        let Some(addr) = entry.addr else { continue };
        let id = if protocol == ConnectionProtocol::Removent {
            if entry.short_fp.is_empty() || entry.short_fp == my_fp {
                continue;
            }
            entry.short_fp.clone()
        } else {
            // Service aliases advertising the same endpoint collapse; protocols
            // on the same host never share an identity or pairing trust.
            format!("{}:{addr}", protocol.short_label())
        };
        if entries.get(&id).is_none_or(|current| {
            // Keep the advertised name when a port probe finds the same server.
            (!entry.instance.starts_with("scan:"), entry.seen_at)
                > (!current.instance.starts_with("scan:"), current.seen_at)
        }) {
            entries.insert(id, entry);
        }
    }
    entries
        .into_iter()
        .map(|(id, entry)| (id, (entry.name.clone(), entry.addr.unwrap())))
        .collect()
}

fn publish(
    protocol: ConnectionProtocol,
    previous: &VisibleDevices,
    next: &VisibleDevices,
    events: &Sender<UiEvent>,
) {
    for (id, target) in next {
        let (name, addr) = target;
        if previous.get(id) != Some(target) {
            let _ = events.send(UiEvent::DeviceFound {
                fp: id.clone(),
                name: name.clone(),
                addr: *addr,
                protocol,
            });
        }
    }
    for id in previous.keys().filter(|id| !next.contains_key(*id)) {
        let _ = events.send(UiEvent::DeviceLost(id.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use removent_proto::Caps;
    use std::time::Instant;

    #[tokio::test]
    async fn toggles_start_stop_and_restart_only_the_selected_browser() {
        let (settings, rx) = watch::channel(DiscoverySettings::default());
        let (events, _rx) = std::sync::mpsc::channel();
        let observers = std::sync::Mutex::new(Vec::new());
        let scans = std::sync::atomic::AtomicUsize::new(0);
        let mut worker = Box::pin(run_with_browser(
            ConnectionProtocol::Vnc,
            rx,
            String::new(),
            events,
            |protocol| {
                let browser = DiscoveryBrowser::start_for(protocol)?;
                observers.lock().unwrap().push(browser.subscribe_table());
                Ok(browser)
            },
            |_| {
                scans.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                None
            },
        ));
        assert!(futures::poll!(&mut worker).is_pending());
        assert_eq!(scans.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(
            observers.lock().unwrap().is_empty(),
            "disabled discovery must not open a browser"
        );
        settings.send_modify(|s| s.vnc = true);
        assert!(futures::poll!(&mut worker).is_pending());
        assert_eq!(observers.lock().unwrap().len(), 1);
        assert_eq!(scans.load(std::sync::atomic::Ordering::SeqCst), 1);
        settings.send_modify(|s| s.rdp = true);
        assert!(futures::poll!(&mut worker).is_pending());
        assert_eq!(
            observers.lock().unwrap().len(),
            1,
            "unrelated toggle must not restart VNC"
        );
        settings.send_modify(|s| s.vnc = false);
        assert_eq!(scans.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(futures::poll!(&mut worker).is_pending());
        let mut stopped = observers.lock().unwrap()[0].clone();
        tokio::time::timeout(Duration::from_secs(2), async {
            while stopped.changed().await.is_ok() {}
        })
        .await
        .expect("disable must stop the mDNS worker");
        settings.send_modify(|s| s.vnc = true);
        assert!(futures::poll!(&mut worker).is_pending());
        assert_eq!(observers.lock().unwrap().len(), 2);
        assert_eq!(scans.load(std::sync::atomic::Ordering::SeqCst), 2);
        drop(settings);
        tokio::time::timeout(Duration::from_secs(2), worker)
            .await
            .unwrap();
    }

    fn entry(instance: &str, fp: &str, addr: Option<&str>) -> DeviceEntry {
        DeviceEntry {
            instance: instance.into(),
            name: instance.into(),
            short_fp: fp.into(),
            addr: addr.map(|s| s.parse().unwrap()),
            caps: Caps::all(),
            busy: false,
            seen_at: Instant::now(),
        }
    }

    #[test]
    fn native_deduplicates_identity_and_filters_self_and_unresolved_entries() {
        let older = entry("old", "peer", Some("192.168.1.2:48688"));
        let newer = entry("new", "peer", Some("192.168.1.3:48688"));
        let table = [
            older,
            newer,
            entry("unresolved", "peer", None),
            entry("self", "mine", Some("192.168.1.4:48688")),
            entry("no identity", "", Some("192.168.1.5:48688")),
        ]
        .into_iter()
        .map(|e| (e.instance.clone(), e))
        .collect();
        let visible = visible_devices(ConnectionProtocol::Removent, "mine", &table);
        assert_eq!(visible.len(), 1);
        assert_eq!(
            visible["peer"],
            ("new".into(), "192.168.1.3:48688".parse().unwrap())
        );
    }

    #[test]
    fn compatibility_aliases_deduplicate_but_protocols_and_ports_remain_distinct() {
        let table = [
            entry("rdp", "", Some("192.168.1.2:3389")),
            entry("ms-wbt-server", "", Some("192.168.1.2:3389")),
            entry("another desktop", "", Some("192.168.1.2:3390")),
            entry("scan:192.168.1.2:3389", "", Some("192.168.1.2:3389")),
        ]
        .into_iter()
        .map(|e| (e.instance.clone(), e))
        .collect();
        let rdp = visible_devices(ConnectionProtocol::Rdp, "", &table);
        let vnc = visible_devices(ConnectionProtocol::Vnc, "", &table);
        assert_eq!(rdp.len(), 2);
        assert_eq!(vnc.len(), 2);
        assert_eq!(rdp["RDP:192.168.1.2:3389"].0, "ms-wbt-server");
        assert!(rdp.keys().all(|key| !vnc.contains_key(key)));
    }

    #[test]
    fn rename_address_change_and_removal_refresh_the_ui() {
        let protocol = ConnectionProtocol::Removent;
        let (tx, rx) = std::sync::mpsc::channel();
        let first = BTreeMap::from([(
            "peer".into(),
            ("Old".into(), "192.168.1.2:48688".parse().unwrap()),
        )]);
        let next = BTreeMap::from([(
            "peer".into(),
            ("New".into(), "192.168.1.3:48688".parse().unwrap()),
        )]);
        publish(protocol, &BTreeMap::new(), &first, &tx);
        publish(protocol, &first, &first, &tx);
        publish(protocol, &first, &next, &tx);
        publish(protocol, &next, &BTreeMap::new(), &tx);
        let events: Vec<_> = rx.try_iter().collect();
        assert_eq!(events.len(), 3);
        assert!(
            matches!(&events[1], UiEvent::DeviceFound { name, addr, .. } if name == "New" && addr.ip().to_string() == "192.168.1.3")
        );
        assert!(matches!(&events[2], UiEvent::DeviceLost(id) if id == "peer"));
    }
}
