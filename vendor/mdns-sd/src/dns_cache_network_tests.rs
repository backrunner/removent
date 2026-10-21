use super::*;
use crate::dns_parser::{DnsNSec, DnsRecordExt, DnsSrv, DnsTxt, CLASS_CACHE_FLUSH, CLASS_IN};

const SERVICE: &str = "_multi._tcp.local.";
const INSTANCE: &str = "shared._multi._tcp.local.";
const HOST: &str = "shared.local.";

fn interface(name: &str, index: u32) -> MyIntf {
    MyIntf {
        name: name.into(),
        index,
        addrs: HashSet::new(),
        max_packet_size_v4: crate::MAX_PKT_DEFAULT,
        max_packet_size_v6: crate::MAX_PKT_DEFAULT,
    }
}

fn advertise(cache: &mut DnsCache, intf: &MyIntf, ttl: u32) {
    for record in [
        DnsPointer::new(SERVICE, RRType::PTR, CLASS_IN, ttl, INSTANCE.into()).boxed(),
        DnsSrv::new(INSTANCE, CLASS_IN, ttl, 0, 0, 80, HOST.into()).boxed(),
        DnsTxt::new(INSTANCE, CLASS_IN, ttl, vec![]).boxed(),
        DnsAddress::new(
            HOST,
            RRType::A,
            CLASS_IN,
            ttl,
            "192.168.1.22".parse().unwrap(),
            intf.into(),
        )
        .boxed(),
    ] {
        cache.add_or_update(intf, record, &mut Vec::new(), true);
    }
}

#[test]
fn service_is_removed_only_after_its_last_interface_disappears() {
    for first_removed in [0, 1] {
        let intfs = [interface("ethernet", 8), interface("wifi", 19)];
        let mut cache = DnsCache::new();
        for intf in &intfs {
            advertise(&mut cache, intf, 120);
        }
        let result = cache.remove_records_on_intf((&intfs[first_removed]).into());
        assert!(
            result.removed_instances.is_empty(),
            "service still exists on another interface, removal order {}: {:?}",
            first_removed,
            result.removed_instances
        );
        assert!(result.modified_instances.contains(INSTANCE));
        assert_eq!(cache.get_ptr(SERVICE).unwrap().len(), 1);
        assert_eq!(cache.get_srv(INSTANCE).unwrap().len(), 1);
        assert_eq!(cache.txt_count(), 1);
        let addresses = cache.get_addresses_for_host(HOST);
        let expected = DnsAddress::new(
            HOST,
            RRType::A,
            CLASS_IN,
            120,
            "192.168.1.22".parse().unwrap(),
            (&intfs[1 - first_removed]).into(),
        )
        .address();
        assert_eq!(addresses[HOST], HashSet::from([expected]));

        let result = cache.remove_records_on_intf((&intfs[1 - first_removed]).into());
        assert_eq!(
            result.removed_instances[SERVICE],
            HashSet::from([INSTANCE.into()])
        );
        assert!(cache.get_ptr(SERVICE).is_none());
        assert!(cache.get_srv(INSTANCE).is_none());
        assert_eq!(cache.txt_count(), 0);
        assert!(cache.get_addresses_for_host(HOST).is_empty());
    }
}

#[test]
fn expiry_on_one_interface_preserves_the_other_interfaces_records() {
    for short_lived in [0, 1] {
        let mut cache = DnsCache::new();
        let intfs = [interface("ethernet", 8), interface("wifi", 19)];
        for (i, intf) in intfs.iter().enumerate() {
            advertise(&mut cache, intf, if i == short_lived { 1 } else { 120 });
        }
        let now = Instant::now();
        assert!(cache
            .evict_expired_services(now + Duration::from_secs(2))
            .is_empty());
        assert_eq!(cache.ptr_count(), 1);
        assert_eq!(
            cache.get_ptr(SERVICE).unwrap()[0].src_intf.index,
            intfs[1 - short_lived].index
        );
        assert_eq!(cache.srv_count(), 1);
        assert_eq!(cache.txt_count(), 1);
        let removed = cache.evict_expired_services(now + Duration::from_secs(121));
        assert_eq!(removed[SERVICE], HashSet::from([INSTANCE.into()]));
        assert!(cache
            .evict_expired_services(now + Duration::from_secs(122))
            .is_empty());
        assert_eq!(cache.ptr_count(), 0);
        assert_eq!(cache.srv_count(), 0);
        assert_eq!(cache.txt_count(), 0);
    }
}

#[test]
fn refreshes_do_not_grow_the_cache_per_interface() {
    let mut cache = DnsCache::new();
    for _ in 0..1_000 {
        advertise(&mut cache, &interface("ethernet", 8), 120);
        advertise(&mut cache, &interface("wifi", 19), 120);
    }
    assert_eq!(cache.ptr_count(), 2);
    assert_eq!(cache.srv_count(), 2);
    assert_eq!(cache.txt_count(), 2);
    assert_eq!(cache.addr_count(), 2);
}

#[test]
fn cache_flush_only_expires_records_from_the_receiving_interface() {
    let mut cache = DnsCache::new();
    let intfs = [interface("ethernet", 8), interface("wifi", 19)];
    for intf in &intfs {
        advertise(&mut cache, intf, 120);
        cache.add_or_update(
            intf,
            DnsNSec::new(INSTANCE, CLASS_IN, 120, INSTANCE.into(), vec![0, 1, 0x40]).boxed(),
            &mut Vec::new(),
            true,
        );
    }
    cache.age_records(Duration::from_secs(2));
    let class = CLASS_IN | CLASS_CACHE_FLUSH;
    for record in [
        DnsSrv::new(INSTANCE, class, 120, 0, 0, 81, HOST.into()).boxed(),
        DnsTxt::new(INSTANCE, class, 120, vec![3, b'x', b'=', b'y']).boxed(),
        DnsNSec::new(INSTANCE, class, 120, INSTANCE.into(), vec![0, 1, 0x20]).boxed(),
    ] {
        cache.add_or_update(&intfs[0], record, &mut Vec::new(), true);
    }
    cache.evict_expired_services(Instant::now() + Duration::from_secs(2));
    assert_eq!(cache.srv_count(), 2);
    assert_eq!(cache.txt_count(), 2);
    assert_eq!(cache.nsec_count(), 2);
    let ports: HashMap<_, _> = cache
        .get_srv(INSTANCE)
        .unwrap()
        .iter()
        .map(|r| {
            (
                r.src_intf.index,
                r.record.any().downcast_ref::<DnsSrv>().unwrap().port(),
            )
        })
        .collect();
    assert_eq!(ports, HashMap::from([(8, 81), (19, 80)]));
}
