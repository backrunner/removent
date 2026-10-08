use super::*;

#[test]
fn native_advertisement_has_one_dns_sd_instance_label() {
    let name = "电脑.工作站".repeat(20);
    let info = Advertiser::build_info(
        "host".into(),
        &name,
        "0123456789abcdef",
        48688,
        Caps::all(),
        false,
        None,
    )
    .unwrap();
    let label = info
        .get_fullname()
        .strip_suffix(&format!(".{MDNS_SERVICE}"))
        .unwrap();
    assert!(!label.contains('.'));
    assert!(label.len() <= 63);
    assert_eq!(info.get_property_val_str("name"), Some(name.as_str()));
}

#[test]
fn locator_counts_device_identities_across_bonjour_aliases() {
    let ad = removent_core::pairing_invitation::Advertisement {
        locator: "123456".into(),
        expires_at_unix: removent_core::pairing_invitation::now() + 300,
    };
    let info = Advertiser::build_info(
        "host".into(),
        "Computer",
        "0123456789abcdef",
        48688,
        Caps::all(),
        false,
        Some(&ad),
    )
    .unwrap();
    let mut first = resolved_entry(DiscoveryProtocol::Removent, &info).unwrap();
    first.addr = Some("127.0.0.1:48688".parse().unwrap());
    let mut second = first.clone();
    second.instance += " (2)";
    let mut table = HashMap::from([
        (first.instance.clone(), first),
        (second.instance.clone(), second),
    ]);
    assert_eq!(pairing_destinations(&table, "123456").len(), 1);
    table.values_mut().next().unwrap().short_fp = "fedcba9876543210".into();
    assert_eq!(pairing_destinations(&table, "123456").len(), 2);
}

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
            // Other native advertisers may be running on the LAN or in parallel tests.
            // Verify only that this unique compatibility service does not leak into it.
            assert!(
                !native
                    .subscribe_table()
                    .borrow()
                    .values()
                    .any(|entry| entry.name == name)
            );
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
    let advertiser = Advertiser::start("DropTest", "0000000000000001", 48699, Caps::all()).unwrap();
    let daemon = advertiser.daemon.0.clone();
    drop(advertiser);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match daemon.status() {
                Ok(status) => {
                    // A status command queued behind Exit may never be
                    // processed; retry status() to observe disconnection.
                    if matches!(
                        tokio::time::timeout(Duration::from_millis(50), status.recv_async()).await,
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

    let ad = removent_core::pairing_invitation::Advertisement {
        locator: "123456".into(),
        expires_at_unix: removent_core::pairing_invitation::now() + 300,
    };
    adv.set_pairing(Some(ad.clone())).unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if rx
                .borrow_and_update()
                .values()
                .any(|entry| entry.short_fp == fp && entry.pairing == Some(ad.clone()))
            {
                break;
            }
            rx.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    for busy in [true, false, true, false] {
        adv.set_busy(busy).unwrap();
        assert_eq!(
            wait_for_entry(&mut rx, &fp, busy, true).await.pairing,
            Some(ad.clone())
        );
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
