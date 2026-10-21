use super::*;
use crate::dns_parser::{DnsRecordExt, DnsSrv, DnsTxt, CLASS_IN};

const SERVICE: &str = "_memory._tcp.local.";
const INSTANCE: &str = "test._memory._tcp.local.";

fn daemon() -> (Zeroconf, u32) {
    let signal = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let signal_addr = signal.local_addr().unwrap();
    signal.set_nonblocking(true).unwrap();
    let reservation = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = reservation.local_addr().unwrap().port();
    drop(reservation);
    let (sender, _) = flume::bounded(100);
    let mut daemon = Zeroconf::new(
        MioUdpSocket::from_std(signal),
        Poll::new().unwrap(),
        port,
        sender,
        signal_addr,
    );
    assert!(
        daemon.ipv4_sock.is_some(),
        "test daemon must bind its IPv4 socket"
    );
    let index = my_ip_interfaces(true)
        .iter()
        .find(|intf| intf.ip() == IpAddr::V4(Ipv4Addr::LOCALHOST))
        .unwrap()
        .index
        .unwrap();
    daemon.my_intfs.retain(|key, _| *key == index);
    daemon.dns_registry_map.retain(|key, _| *key == index);
    (daemon, index)
}

fn response(index: u32, records: Vec<DnsRecordBox>) -> DnsIncoming {
    let mut outgoing = DnsOutgoing::new(FLAGS_QR_RESPONSE | FLAGS_AA);
    for record in records {
        outgoing.add_answer_box(record);
    }
    let packets = outgoing.to_data_on_wire(crate::MAX_PKT_DEFAULT, true);
    assert_eq!(packets.len(), 1);
    DnsIncoming::new(
        packets[0].clone(),
        InterfaceId {
            name: "test".into(),
            index,
        },
    )
    .unwrap()
}

fn records(instance: &str) -> Vec<DnsRecordBox> {
    vec![
        DnsSrv::new(instance, CLASS_IN, 60, 0, 0, 80, "host.local.".into()).boxed(),
        DnsTxt::new(instance, CLASS_IN, 60, vec![3, b'a', b'=', b'b']).boxed(),
    ]
}

#[test]
fn publisher_does_not_accumulate_responses_or_cache_timers() {
    let (mut daemon, index) = daemon();
    daemon.register_service(
        ServiceInfo::new(
            SERVICE,
            "publisher",
            "publisher.local.",
            "127.0.0.1",
            8080,
            None,
        )
        .unwrap(),
    );
    let initial_timers = daemon.timers.len();
    for i in 0..2_000 {
        let msg = response(index, records(&format!("other-{i}.{SERVICE}")));
        daemon.handle_response(msg, index);
    }
    assert_eq!(daemon.cache.srv_count(), 0);
    assert_eq!(daemon.cache.txt_count(), 0);
    assert_eq!(daemon.timers.len(), initial_timers);
}

#[test]
fn cache_consumers_still_receive_responses_without_ptr() {
    for mode in ["browse", "hostname", "unsolicited"] {
        let (mut daemon, index) = daemon();
        // Keep receiver ends alive; each mode is an independent cache consumer.
        let (services, _service_events) = flume::bounded(10);
        let (hosts, _host_events) = flume::bounded(10);
        match mode {
            "browse" => {
                daemon.service_queriers.insert(SERVICE.into(), services);
            }
            "hostname" => {
                daemon
                    .hostname_resolvers
                    .insert("host.local.".into(), (hosts, None));
            }
            _ => daemon.accept_unsolicited = true,
        }
        daemon.handle_response(response(index, records(INSTANCE)), index);
        assert_eq!(daemon.cache.srv_count(), 1, "mode={mode}");
        assert_eq!(daemon.cache.txt_count(), 1, "mode={mode}");
    }
}

#[test]
fn publisher_detects_name_conflicts_before_skipping_cache() {
    let (mut daemon, index) = daemon();
    let info = ServiceInfo::new(SERVICE, "test", "host.local.", "127.0.0.1", 8080, None).unwrap();
    daemon.register_service(info);
    assert!(daemon.dns_registry_map[&index]
        .probing
        .contains_key(INSTANCE));
    // Another publisher uses the same instance with a different SRV port.
    daemon.handle_response(response(index, records(INSTANCE)), index);
    assert!(daemon.dns_registry_map[&index]
        .name_changes
        .contains_key(INSTANCE));
    assert_eq!(daemon.cache.srv_count(), 0);
    assert_eq!(daemon.cache.txt_count(), 0);
}
