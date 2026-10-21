use super::*;
use crate::dns_parser::{DnsNSec, DnsRecordExt, DnsSrv, DnsTxt, CLASS_IN};

const SERVICE: &str = "_memory._tcp.local.";
const INSTANCE: &str = "test._memory._tcp.local.";

fn intf() -> MyIntf {
    MyIntf {
        name: "test".into(),
        index: 1,
        addrs: HashSet::new(),
        max_packet_size_v4: crate::MAX_PKT_DEFAULT,
        max_packet_size_v6: crate::MAX_PKT_DEFAULT,
    }
}

fn add(cache: &mut DnsCache, record: impl DnsRecordExt + 'static) {
    cache.add_or_update(&intf(), record.boxed(), &mut Vec::new(), true);
}

fn ptr(cache: &mut DnsCache, name: &str, instance: &str, ttl: u32) {
    add(
        cache,
        DnsPointer::new(name, RRType::PTR, CLASS_IN, ttl, instance.into()),
    );
}

fn srv(cache: &mut DnsCache, instance: &str, ttl: u32) {
    add(
        cache,
        DnsSrv::new(instance, CLASS_IN, ttl, 0, 0, 80, "host.local.".into()),
    );
}

#[test]
fn orphan_records_expire_and_churn_returns_table_capacity() {
    let mut cache = DnsCache::new();
    for round in 0..3 {
        for i in 0..2_000 {
            let instance = format!("instance-{round}-{i}.{SERVICE}");
            srv(&mut cache, &instance, 1);
            add(
                &mut cache,
                DnsTxt::new(&instance, CLASS_IN, 1, vec![3, b'a', b'=', b'b']),
            );
            add(
                &mut cache,
                DnsNSec::new(&instance, CLASS_IN, 1, instance.clone(), vec![0, 1, 0x40]),
            );
        }
        assert_eq!(cache.srv_count(), 2_000);
        assert_eq!(cache.txt_count(), 2_000);
        assert_eq!(cache.nsec_count(), 2_000);
        // No real sleeps: use the same expiry path with a future monotonic time.
        assert!(cache
            .evict_expired_services(Instant::now() + Duration::from_secs(2))
            .is_empty());
        for table in [&cache.ptr, &cache.srv, &cache.txt, &cache.nsec] {
            assert!(table.is_empty());
            assert_eq!(
                table.capacity(),
                0,
                "retained peak capacity after round {round}"
            );
        }
    }
}

#[test]
fn expiry_wave_releases_capacity_while_preserving_live_records() {
    let mut cache = DnsCache::new();
    for i in 0..2_000 {
        srv(&mut cache, &format!("temporary-{i}.{SERVICE}"), 1);
    }
    srv(&mut cache, INSTANCE, 100);
    cache.evict_expired_services(Instant::now() + Duration::from_secs(2));
    assert_eq!(cache.srv_count(), 1);
    assert!(cache.get_srv(INSTANCE).is_some());
    assert!(cache.srv.capacity() <= 64, "peak allocation was retained");
}

#[test]
fn ptr_expiry_notifies_once_and_orphans_expire_later() {
    let mut cache = DnsCache::new();
    ptr(&mut cache, SERVICE, INSTANCE, 1);
    srv(&mut cache, INSTANCE, 10);
    add(&mut cache, DnsTxt::new(INSTANCE, CLASS_IN, 10, vec![]));
    let now = Instant::now();
    let expired = cache.evict_expired_services(now + Duration::from_secs(2));
    assert_eq!(expired[SERVICE], HashSet::from([INSTANCE.to_owned()]));
    assert!(cache.ptr.is_empty());
    assert_eq!(cache.srv_count(), 1);
    assert_eq!(cache.txt_count(), 1);
    assert!(cache
        .evict_expired_services(now + Duration::from_secs(3))
        .is_empty());
    assert!(cache
        .evict_expired_services(now + Duration::from_secs(11))
        .is_empty());
    assert!(cache.srv.is_empty());
    assert!(cache.txt.is_empty());
}

#[test]
fn srv_expiry_preserves_removal_event_and_live_ptr() {
    let mut cache = DnsCache::new();
    ptr(&mut cache, SERVICE, INSTANCE, 10);
    srv(&mut cache, INSTANCE, 1);
    let now = Instant::now();
    let expired = cache.evict_expired_services(now + Duration::from_secs(2));
    assert_eq!(expired[SERVICE], HashSet::from([INSTANCE.to_owned()]));
    assert_eq!(cache.ptr_count(), 1);
    assert!(cache.srv.is_empty());
    assert!(cache
        .evict_expired_services(now + Duration::from_secs(3))
        .is_empty());
}

#[test]
fn refreshed_and_live_records_survive_expiry_sweep() {
    let mut cache = DnsCache::new();
    ptr(&mut cache, SERVICE, INSTANCE, 100);
    srv(&mut cache, INSTANCE, 1);
    srv(&mut cache, INSTANCE, 100);
    add(&mut cache, DnsTxt::new(INSTANCE, CLASS_IN, 1, vec![]));
    assert!(cache
        .evict_expired_services(Instant::now() + Duration::from_secs(2))
        .is_empty());
    assert_eq!(cache.srv_count(), 1);
    assert_eq!(cache.ptr_count(), 1);
    assert!(cache.txt.is_empty());
}

#[test]
fn subtype_lifetime_follows_ptr_without_requiring_srv() {
    let mut cache = DnsCache::new();
    let subtype = "_printer._sub._memory._tcp.local.";
    ptr(&mut cache, subtype, INSTANCE, 10);
    let now = Instant::now();
    cache.evict_expired_services(now + Duration::from_secs(1));
    assert_eq!(
        cache.get_subtype(INSTANCE).map(String::as_str),
        Some(subtype)
    );
    cache.evict_expired_services(now + Duration::from_secs(11));
    assert!(cache.ptr.is_empty());
    assert!(cache.subtype.is_empty());
}

#[test]
fn expired_addresses_report_removal_and_release_capacity() {
    let mut cache = DnsCache::new();
    for i in 0..2_000 {
        add(
            &mut cache,
            DnsAddress::new(
                &format!("host-{i}.local."),
                RRType::A,
                CLASS_IN,
                1,
                "127.0.0.1".parse().unwrap(),
                InterfaceId {
                    name: "test".into(),
                    index: 1,
                },
            ),
        );
    }
    let expired = cache.evict_expired_addr(Instant::now() + Duration::from_secs(2));
    assert_eq!(expired.len(), 2_000);
    assert!(cache.addr.is_empty());
    assert_eq!(cache.addr.capacity(), 0);
}
