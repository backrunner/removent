//! Bounded, cancellable discovery of VNC/RDP servers without DNS-SD advertising.
//! Only directly connected IPv4 subnets are visited; probes stop before login.

use crate::{DeviceEntry, DiscoveryProtocol};
use futures::{StreamExt, stream};
use removent_proto::Caps;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpSocket;
use tokio::sync::watch;

const BATCH_PER_SUBNET: u64 = 256;
const CONCURRENT_PROBES: usize = 32;
const PROBE_TIMEOUT: Duration = Duration::from_millis(1200);
const SCAN_INTERVAL: Duration = Duration::from_secs(30);
// TPKT + X.224 Connection Request + RDP_NEG_REQ (TLS and CredSSP).
const RDP_REQUEST: &[u8] = &[3, 0, 0, 19, 14, 0xe0, 0, 0, 0, 0, 0, 1, 0, 8, 0, 3, 0, 0, 0];

type Table = HashMap<String, DeviceEntry>;

pub struct LanScanner {
    task: tokio::task::JoinHandle<()>,
    table: watch::Receiver<Arc<Table>>,
}

impl LanScanner {
    pub fn start(protocol: DiscoveryProtocol) -> Option<Self> {
        let port = match protocol {
            DiscoveryProtocol::Removent => return None,
            DiscoveryProtocol::Vnc => 5900,
            DiscoveryProtocol::Rdp => 3389,
        };
        let (tx, table) = watch::channel(Arc::new(Table::new()));
        let task = tokio::spawn(async move {
            let mut entries = Table::new();
            let mut cursors = BTreeMap::new();
            loop {
                match local_subnets() {
                    Ok(subnets) => {
                        let targets = targets(&subnets, &mut cursors, &entries, port);
                        entries.retain(|_, entry| {
                            entry
                                .addr
                                .is_some_and(|addr| targets.iter().any(|t| t.remote == addr))
                        });
                        tx.send_replace(Arc::new(entries.clone()));
                        scan_round(protocol, targets, &mut entries, &tx).await;
                    }
                    Err(error) => tracing::warn!(%error, "could not enumerate LAN interfaces"),
                }
                tokio::time::sleep(SCAN_INTERVAL).await;
            }
        });
        Some(Self { task, table })
    }

    pub fn subscribe_table(&self) -> watch::Receiver<Arc<Table>> {
        self.table.clone()
    }
}

impl Drop for LanScanner {
    fn drop(&mut self) {
        // Probes are child futures, not detached tasks. Abort closes all sockets.
        self.task.abort();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Subnet {
    local: Ipv4Addr,
    mask: u32,
}

impl Subnet {
    fn host_range(self) -> Option<(u64, u64)> {
        let prefix = self.mask.leading_ones();
        if prefix == 0 || self.mask != u32::MAX.checked_shl(32 - prefix).unwrap_or(0) {
            return None;
        }
        let first = (u32::from(self.local) & self.mask) as u64;
        let last = first + (!self.mask) as u64;
        if prefix < 31 {
            Some((first + 1, last - 1))
        } else {
            Some((first, last))
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Target {
    local: Ipv4Addr,
    remote: SocketAddr,
}

fn targets(
    subnets: &[Subnet],
    cursors: &mut BTreeMap<Subnet, u64>,
    known: &Table,
    port: u16,
) -> Vec<Target> {
    cursors.retain(|subnet, _| subnets.contains(subnet));
    let local: BTreeSet<_> = subnets.iter().map(|s| s.local).collect();
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for subnet in subnets {
        let Some((first, last)) = subnet.host_range() else {
            continue;
        };
        let mut push = |ip: Ipv4Addr| {
            let numeric = u32::from(ip) as u64;
            if (first..=last).contains(&numeric)
                && !local.contains(&ip)
                && !ip.is_multicast()
                && !ip.is_loopback()
                && !ip.is_unspecified()
                && !ip.is_broadcast()
                && seen.insert(ip)
            {
                result.push(Target {
                    local: subnet.local,
                    remote: (ip, port).into(),
                });
            }
        };
        // Known servers are checked every round, even while larger subnets are
        // explored gradually. Network changes remove stale routes immediately.
        for entry in known.values() {
            if let Some(SocketAddr::V4(addr)) = entry.addr {
                push(*addr.ip());
            }
        }
        // Revisit the local /24 each round so newly started nearby servers in
        // a large subnet do not wait for an entire subnet traversal.
        let local_block = (u32::from(subnet.local) & 0xffff_ff00) as u64;
        for address in first.max(local_block)..=last.min(local_block + 255) {
            push(Ipv4Addr::from(address as u32));
        }
        let count = last - first + 1;
        let offset = cursors.entry(*subnet).or_insert_with(|| {
            ((u32::from(subnet.local) & 0xffff_ff00) as u64).saturating_sub(first) % count
        });
        for _ in 0..BATCH_PER_SUBNET.min(count) {
            push(Ipv4Addr::from((first + *offset) as u32));
            *offset = (*offset + 1) % count;
        }
    }
    result
}

fn local_subnets() -> std::io::Result<Vec<Subnet>> {
    let mut head = std::ptr::null_mut();
    // getifaddrs owns this linked list until freeifaddrs; every cast below is
    // guarded by a non-null AF_INET address and netmask.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    struct Interfaces(*mut libc::ifaddrs);
    impl Drop for Interfaces {
        fn drop(&mut self) {
            unsafe { libc::freeifaddrs(self.0) };
        }
    }
    let _interfaces = Interfaces(head);
    let mut next = head;
    let mut result = BTreeSet::new();
    while let Some(interface) = unsafe { next.as_ref() } {
        next = interface.ifa_next;
        let flags = interface.ifa_flags as i32;
        if flags & libc::IFF_UP == 0 || flags & (libc::IFF_LOOPBACK | libc::IFF_POINTOPOINT) != 0 {
            continue;
        }
        if interface.ifa_addr.is_null() || interface.ifa_netmask.is_null() {
            continue;
        }
        let name = unsafe { std::ffi::CStr::from_ptr(interface.ifa_name) }.to_bytes();
        // Apple peer-to-peer Wi-Fi interfaces are not the connected LAN.
        if name.starts_with(b"awdl") || name.starts_with(b"llw") {
            continue;
        }
        unsafe {
            if (*interface.ifa_addr).sa_family as i32 != libc::AF_INET
                || (*interface.ifa_netmask).sa_family as i32 != libc::AF_INET
            {
                continue;
            }
            let address = &*interface.ifa_addr.cast::<libc::sockaddr_in>();
            let mask = &*interface.ifa_netmask.cast::<libc::sockaddr_in>();
            let local = Ipv4Addr::from(u32::from_be(address.sin_addr.s_addr));
            if local.is_unspecified() || local.is_loopback() || local.is_multicast() {
                continue;
            }
            result.insert(Subnet {
                local,
                mask: u32::from_be(mask.sin_addr.s_addr),
            });
        }
    }
    Ok(result.into_iter().collect())
}

async fn scan_round(
    protocol: DiscoveryProtocol,
    targets: Vec<Target>,
    entries: &mut Table,
    tx: &watch::Sender<Arc<Table>>,
) {
    let mut probes = stream::iter(targets)
        .map(|target| async move { (target, probe(protocol, target).await) })
        .buffer_unordered(CONCURRENT_PROBES);
    while let Some((target, found)) = probes.next().await {
        let id = format!("scan:{}", target.remote);
        if found {
            entries.insert(
                id.clone(),
                DeviceEntry {
                    instance: id,
                    name: target.remote.ip().to_string(),
                    short_fp: String::new(),
                    addr: Some(target.remote),
                    caps: Caps::none(),
                    busy: false,
                    seen_at: Instant::now(),
                },
            );
        } else if entries.remove(&id).is_none() {
            continue;
        }
        tx.send_replace(Arc::new(entries.clone()));
    }
}

async fn probe(protocol: DiscoveryProtocol, target: Target) -> bool {
    tokio::time::timeout(PROBE_TIMEOUT, async {
        let socket = TcpSocket::new_v4()?;
        socket.bind((target.local, 0).into())?;
        let mut tcp = socket.connect(target.remote).await?;
        match protocol {
            DiscoveryProtocol::Vnc => {
                let mut greeting = [0; 12];
                tcp.read_exact(&mut greeting).await?;
                Ok(greeting.starts_with(b"RFB 003.")
                    && greeting[8..11].iter().all(u8::is_ascii_digit)
                    && greeting[11] == b'\n')
            }
            DiscoveryProtocol::Rdp => {
                tcp.write_all(RDP_REQUEST).await?;
                let mut header = [0; 4];
                tcp.read_exact(&mut header).await?;
                let len = u16::from_be_bytes([header[2], header[3]]) as usize;
                if header[..2] != [3, 0] || !matches!(len, 11 | 19) {
                    return Ok(false);
                }
                let mut body = [0; 15];
                tcp.read_exact(&mut body[..len - 4]).await?;
                Ok(rdp_confirm(&body[..len - 4]))
            }
            DiscoveryProtocol::Removent => Ok(false),
        }
    })
    .await
    .is_ok_and(|result: std::io::Result<bool>| result.unwrap_or(false))
}

fn rdp_confirm(body: &[u8]) -> bool {
    if !matches!(body.len(), 7 | 15)
        || body[0] as usize + 1 != body.len()
        || body[1] != 0xd0
        || body[6] != 0
    {
        return false;
    }
    if body.len() == 7 {
        return true;
    } // Legacy RDP Connection Confirm.
    if body[9..11] != [8, 0] {
        return false;
    }
    let value = u32::from_le_bytes(body[11..15].try_into().unwrap());
    match body[7] {
        2 => matches!(value, 0 | 1 | 2 | 8),
        3 => (1..=6).contains(&value), // A negotiation refusal still identifies RDP.
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    fn subnet(local: &str, prefix: u32) -> Subnet {
        Subnet {
            local: local.parse().unwrap(),
            mask: u32::MAX << (32 - prefix),
        }
    }

    #[test]
    fn scan_is_bounded_to_direct_subnets_and_excludes_local_and_broadcast_addresses() {
        let subnets = [subnet("192.168.4.7", 24), subnet("192.168.4.8", 24)];
        let found = targets(&subnets, &mut BTreeMap::new(), &Table::new(), 5900);
        assert_eq!(found.len(), 252);
        assert!(found.iter().all(|t| {
            let SocketAddr::V4(addr) = t.remote else {
                return false;
            };
            let last = addr.ip().octets()[3];
            addr.ip().octets()[..3] == [192, 168, 4]
                && addr.port() == 5900
                && ![0, 7, 8, 255].contains(&last)
        }));
        assert!(
            targets(
                &[subnet("192.168.4.7", 32)],
                &mut BTreeMap::new(),
                &Table::new(),
                3389
            )
            .is_empty()
        );
        assert!(
            Subnet {
                local: "192.168.4.7".parse().unwrap(),
                mask: 0
            }
            .host_range()
            .is_none()
        );
        assert!(
            Subnet {
                local: "192.168.4.7".parse().unwrap(),
                mask: 0xff00_ff00
            }
            .host_range()
            .is_none()
        );
    }

    #[test]
    fn larger_subnets_rotate_batches_and_recheck_known_servers_each_round() {
        let subnets = [subnet("10.0.40.7", 16)];
        let mut cursors = BTreeMap::new();
        let first = targets(&subnets, &mut cursors, &Table::new(), 3389);
        assert!(first.len() <= BATCH_PER_SUBNET as usize);
        assert!(
            first
                .iter()
                .any(|t| t.remote.ip().to_string() == "10.0.40.8")
        );
        let addr = first[0].remote;
        let known = Table::from([(
            "known".into(),
            DeviceEntry {
                instance: "known".into(),
                name: "PC".into(),
                short_fp: String::new(),
                addr: Some(addr),
                caps: Caps::none(),
                busy: false,
                seen_at: Instant::now(),
            },
        )]);
        let next = targets(&subnets, &mut cursors, &known, 3389);
        assert!(next.iter().any(|t| t.remote == addr));
        assert!(next.iter().any(|t| !first.contains(t)));
        assert!(first.iter().all(|target| next.contains(target)));
        assert!(next.len() <= BATCH_PER_SUBNET as usize + 256 + 1);
        let moved = targets(&[subnet("192.168.2.4", 24)], &mut cursors, &known, 3389);
        assert!(moved.iter().all(|t| t.remote != addr));
        assert_eq!(cursors.len(), 1);
    }

    async fn reply_probe(protocol: DiscoveryProtocol, reply: &'static [u8]) -> bool {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let remote = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut tcp, _) = listener.accept().await.unwrap();
            if protocol == DiscoveryProtocol::Rdp {
                let mut request = [0; 19];
                tcp.read_exact(&mut request).await.unwrap();
                assert_eq!(request, RDP_REQUEST);
            }
            tcp.write_all(reply).await.unwrap();
            // Discovery closes without credentials or a session handshake.
            let mut extra = [0; 1];
            assert_eq!(tcp.read(&mut extra).await.unwrap_or(0), 0);
        });
        let found = probe(
            protocol,
            Target {
                local: Ipv4Addr::LOCALHOST,
                remote,
            },
        )
        .await;
        server.await.unwrap();
        found
    }

    #[tokio::test]
    async fn identifies_vnc_greetings_and_rejects_unrelated_open_ports() {
        assert!(reply_probe(DiscoveryProtocol::Vnc, b"RFB 003.008\n").await);
        assert!(reply_probe(DiscoveryProtocol::Vnc, b"RFB 003.889\n").await);
        assert!(!reply_probe(DiscoveryProtocol::Vnc, b"HTTP/1.1 200").await);
        assert!(!reply_probe(DiscoveryProtocol::Vnc, b"RFB 003.BAD\n").await);
    }

    #[tokio::test]
    async fn identifies_rdp_negotiation_including_nla_refusals_without_logging_in() {
        assert!(
            reply_probe(
                DiscoveryProtocol::Rdp,
                &[
                    3, 0, 0, 19, 14, 0xd0, 0, 0, 0x12, 0x34, 0, 2, 0, 8, 0, 2, 0, 0, 0
                ]
            )
            .await
        );
        assert!(
            reply_probe(
                DiscoveryProtocol::Rdp,
                &[3, 0, 0, 19, 14, 0xd0, 0, 0, 0, 0, 0, 3, 0, 8, 0, 5, 0, 0, 0]
            )
            .await
        );
        assert!(
            reply_probe(
                DiscoveryProtocol::Rdp,
                &[3, 0, 0, 11, 6, 0xd0, 0, 0, 0, 0, 0]
            )
            .await
        );
        assert!(!reply_probe(DiscoveryProtocol::Rdp, b"HTTP/1.1 200 OK\r\n\r\n").await);
        assert!(!reply_probe(DiscoveryProtocol::Rdp, &[3, 0, 0xff, 0xff]).await);
        assert!(!rdp_confirm(&[
            14, 0xe0, 0, 0, 0, 0, 0, 2, 0, 8, 0, 2, 0, 0, 0
        ]));
    }

    #[tokio::test]
    async fn offline_servers_are_removed_on_the_next_scan() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let remote = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut tcp, _) = listener.accept().await.unwrap();
            tcp.write_all(b"RFB 003.008\n").await.unwrap();
        });
        let target = Target {
            local: Ipv4Addr::LOCALHOST,
            remote,
        };
        let mut entries = Table::new();
        let (tx, rx) = watch::channel(Arc::new(Table::new()));
        scan_round(DiscoveryProtocol::Vnc, vec![target], &mut entries, &tx).await;
        server.await.unwrap();
        assert_eq!(rx.borrow().len(), 1);
        scan_round(DiscoveryProtocol::Vnc, vec![target], &mut entries, &tx).await;
        assert!(rx.borrow().is_empty());
    }

    #[tokio::test]
    async fn disabling_scanner_cancels_in_flight_connections() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let remote = listener.local_addr().unwrap();
        let (tx, table) = watch::channel(Arc::new(Table::new()));
        let task = tokio::spawn(async move {
            scan_round(
                DiscoveryProtocol::Vnc,
                vec![Target {
                    local: Ipv4Addr::LOCALHOST,
                    remote,
                }],
                &mut Table::new(),
                &tx,
            )
            .await;
        });
        let scanner = LanScanner { task, table };
        let (mut tcp, _) = listener.accept().await.unwrap();
        drop(scanner);
        let mut byte = [0; 1];
        assert_eq!(
            tokio::time::timeout(Duration::from_millis(500), tcp.read(&mut byte))
                .await
                .unwrap()
                .unwrap(),
            0
        );
    }
}
