use if_addrs::Interface;
use mdns_sd::{IfKind, ServiceDaemon};
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

// Hosted runners may reject mDNS multicast on their physical interface. Keep
// real IPv4/IPv6 multicast and all loopback addresses, including link-local.
pub fn interfaces() -> Vec<Interface> {
    let interfaces = if_addrs::get_if_addrs().expect("enumerate test interfaces");
    let loopback_index = interfaces
        .iter()
        .find(|intf| intf.ip() == IpAddr::V4(Ipv4Addr::LOCALHOST))
        .expect("IPv4 loopback must be available")
        .index
        .expect("loopback interface must have an index");
    let selected: Vec<_> = interfaces
        .into_iter()
        .filter(|intf| intf.is_oper_up() && intf.index == Some(loopback_index))
        .collect();
    assert!(
        selected.iter().any(|intf| intf.ip().is_ipv6()),
        "IPv6 loopback must be available"
    );
    selected
}

pub fn new_daemon() -> mdns_sd::Result<ServiceDaemon> {
    let index = interfaces()[0].index.expect("loopback interface index");
    let daemon = ServiceDaemon::new()?;
    daemon.disable_interface(IfKind::All)?;
    daemon.enable_interface(vec![IfKind::IndexV4(index), IfKind::IndexV6(index)])?;
    daemon.set_multicast_loop_v4(true)?;
    daemon.set_multicast_loop_v6(true)?;
    // FIFO command barrier: interface selection is applied before returning.
    daemon
        .get_metrics()?
        .recv_timeout(Duration::from_secs(3))
        .expect("daemon fixture initialization");
    Ok(daemon)
}
