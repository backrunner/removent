//! Local-only address checks for nearby-device lists. Explicit connections and
//! invitation lookup keep using the unfiltered discovery transport.

use super::DeviceEntry;
use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr, SocketAddrV6};

#[derive(Clone, Default, PartialEq, Eq)]
pub struct LocalDiscoveryAddresses(HashSet<SocketAddr>);

impl LocalDiscoveryAddresses {
    pub fn from_hosts<'a>(hosts: impl IntoIterator<Item = &'a str>) -> Self {
        Self(
            hosts
                .into_iter()
                .filter_map(|host| parse_host(host, 0))
                .map(canonical)
                .collect(),
        )
    }

    /// Enumerate assigned addresses without DNS, port probes or network traffic.
    #[cfg(unix)]
    pub fn load() -> std::io::Result<Self> {
        use std::{ffi::CStr, ptr};
        struct Interfaces(*mut libc::ifaddrs);
        impl Drop for Interfaces {
            fn drop(&mut self) {
                unsafe { libc::freeifaddrs(self.0) };
            }
        }
        let mut head = ptr::null_mut();
        if unsafe { libc::getifaddrs(&mut head) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let interfaces = Interfaces(head);
        let mut current = interfaces.0;
        let mut hosts = Vec::new();
        while !current.is_null() {
            // getifaddrs owns the list and socket addresses until freeifaddrs.
            let interface = unsafe { &*current };
            current = interface.ifa_next;
            if interface.ifa_addr.is_null() {
                continue;
            }
            let family = unsafe { (*interface.ifa_addr).sa_family } as i32;
            let len = match family {
                libc::AF_INET => std::mem::size_of::<libc::sockaddr_in>(),
                libc::AF_INET6 => std::mem::size_of::<libc::sockaddr_in6>(),
                _ => continue,
            };
            let mut host = [0 as libc::c_char; libc::NI_MAXHOST as usize];
            // getnameinfo normalizes BSD link-local scope bytes and preserves
            // the interface zone. NI_NUMERICHOST cannot trigger a DNS lookup.
            if unsafe {
                libc::getnameinfo(
                    interface.ifa_addr,
                    len as libc::socklen_t,
                    host.as_mut_ptr(),
                    host.len() as libc::socklen_t,
                    ptr::null_mut(),
                    0,
                    libc::NI_NUMERICHOST,
                )
            } == 0
            {
                hosts.push(
                    unsafe { CStr::from_ptr(host.as_ptr()) }
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
        let snapshot = Self::from_hosts(hosts.iter().map(String::as_str));
        if snapshot.0.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no local interface addresses",
            ));
        }
        Ok(snapshot)
    }

    #[cfg(not(unix))]
    pub fn load() -> std::io::Result<Self> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "local discovery addresses require Unix",
        ))
    }

    /// Any assigned, routable address identifies our own service. Loopback in
    /// a remote record is discarded instead of hiding its valid remote address.
    pub fn remote_addr(&self, entry: &DeviceEntry) -> Option<SocketAddr> {
        let selected = entry.addr?;
        let mut candidates = vec![selected];
        candidates.extend(
            entry
                .hosts
                .iter()
                .filter_map(|host| parse_host(host, selected.port())),
        );
        self.select(candidates)
    }

    /// Native Bonjour supplies numeric hosts, including IPv6 interface zones.
    pub fn remote_host(&self, hosts: &[String]) -> Option<String> {
        self.select(
            hosts
                .iter()
                .filter_map(|host| parse_host(host, 0))
                .collect(),
        )
        .map(|addr| match addr {
            SocketAddr::V6(v6) if v6.scope_id() != 0 => format!("{}%{}", v6.ip(), v6.scope_id()),
            _ => addr.ip().to_string(),
        })
    }

    fn select(&self, mut candidates: Vec<SocketAddr>) -> Option<SocketAddr> {
        candidates.retain(|addr| usable(*addr));
        if candidates
            .iter()
            .any(|addr| self.0.contains(&canonical(*addr)))
        {
            return None;
        }
        candidates.sort_by_key(|addr| (addr.is_ipv6(), *addr));
        candidates.dedup();
        candidates.into_iter().next()
    }
}

fn canonical(addr: SocketAddr) -> SocketAddr {
    match addr {
        SocketAddr::V6(v6) => {
            if let Some(v4) = v6.ip().to_ipv4_mapped() {
                return SocketAddr::new(v4.into(), 0);
            }
            SocketAddrV6::new(
                *v6.ip(),
                0,
                0,
                if v6.ip().is_unicast_link_local() {
                    v6.scope_id()
                } else {
                    0
                },
            )
            .into()
        }
        SocketAddr::V4(v4) => SocketAddr::new((*v4.ip()).into(), 0),
    }
}

fn usable(addr: SocketAddr) -> bool {
    let addr = canonical(addr);
    let ip = addr.ip();
    !ip.is_loopback()
        && !ip.is_unspecified()
        && !ip.is_multicast()
        && !matches!(ip, IpAddr::V4(v4) if v4.is_broadcast())
        && !matches!(addr, SocketAddr::V6(v6) if v6.ip().is_unicast_link_local() && v6.scope_id() == 0)
}

fn parse_host(host: &str, port: u16) -> Option<SocketAddr> {
    let (ip, scope) = host.split_once('%').unwrap_or((host, "0"));
    let ip: IpAddr = ip.parse().ok()?;
    match ip {
        IpAddr::V4(_) => Some(SocketAddr::new(ip, port)),
        IpAddr::V6(ip) => {
            let scope = scope.parse().ok().or_else(|| {
                #[cfg(unix)]
                {
                    let name = std::ffi::CString::new(scope).ok()?;
                    Some(unsafe { libc::if_nametoindex(name.as_ptr()) })
                }
                #[cfg(not(unix))]
                {
                    None
                }
            })?;
            Some(SocketAddrV6::new(ip, port, 0, scope).into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use removent_proto::Caps;
    fn entry(address: &str, hosts: &[&str]) -> DeviceEntry {
        DeviceEntry {
            pairing: None,
            instance: "same name".into(),
            name: "Mac Studio".into(),
            short_fp: String::new(),
            addr: Some(address.parse().unwrap()),
            hosts: hosts.iter().map(|host| (*host).into()).collect(),
            caps: Caps::none(),
            busy: false,
            seen_at: std::time::Instant::now(),
        }
    }
    #[test]
    fn local_interfaces_mapped_addresses_and_scoped_ipv6_are_excluded() {
        let local =
            LocalDiscoveryAddresses::from_hosts(["192.168.1.4", "2001:db8::4", "fe80::4%8"]);
        for address in [
            "192.168.1.4:5900",
            "[::ffff:192.168.1.4]:3389",
            "[2001:db8::4]:5900",
            "[fe80::4%8]:5900",
            "127.42.0.1:5900",
            "[::ffff:127.0.0.1]:5900",
            "[::1]:5900",
            "0.0.0.0:5900",
            "[::]:5900",
        ] {
            assert!(
                local.remote_addr(&entry(address, &[])).is_none(),
                "{address}"
            );
        }
        assert!(local.remote_addr(&entry("[fe80::4%9]:5900", &[])).is_some());
        assert!(
            local
                .remote_addr(&entry("192.168.1.11:5900", &["192.168.1.4"]))
                .is_none()
        );
    }
    #[test]
    fn remote_peers_keep_valid_addresses_and_names_do_not_define_identity() {
        let local = LocalDiscoveryAddresses::from_hosts(["192.168.1.4"]);
        let remote = entry(
            "127.0.0.1:5900",
            &["mac-studio.local.", "192.168.1.11", "::1"],
        );
        assert_eq!(
            local.remote_addr(&remote),
            Some("192.168.1.11:5900".parse().unwrap())
        );
        assert_eq!(
            local.remote_addr(&entry("[2001:db8::11]:3389", &[])),
            Some("[2001:db8::11]:3389".parse().unwrap())
        );
    }
    #[test]
    #[cfg(unix)]
    fn real_interface_snapshot_filters_each_assigned_address() {
        let local = LocalDiscoveryAddresses::load().unwrap();
        assert!(!local.0.is_empty());
        for addr in &local.0 {
            let mut addr = *addr;
            addr.set_port(5900);
            assert!(local.remote_addr(&entry(&addr.to_string(), &[])).is_none());
        }
    }
}
