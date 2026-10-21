//! Isolated real-UDP cache churn; never sends to production mDNS port 5353.
//! cargo run --manifest-path vendor/mdns-sd/Cargo.toml --locked --example memory_probe
use mdns_sd::{IfKind, Metrics, ServiceDaemon};
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    thread::sleep,
    time::Duration,
};

fn name(value: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    for label in value.trim_end_matches('.').split('.') {
        bytes.push(label.len() as u8);
        bytes.extend(label.as_bytes());
    }
    bytes.push(0);
    bytes
}

fn record(packet: &mut Vec<u8>, owner: &str, ty: u16, data: &[u8]) {
    packet.extend(name(owner));
    packet.extend(ty.to_be_bytes());
    packet.extend(1u16.to_be_bytes());
    packet.extend(1u32.to_be_bytes()); // one-second TTL
    packet.extend((data.len() as u16).to_be_bytes());
    packet.extend(data);
}

fn metrics(daemon: &ServiceDaemon) -> Metrics {
    daemon
        .get_metrics()
        .unwrap()
        .recv_timeout(Duration::from_secs(3))
        .unwrap()
}

fn sample(label: &str, publisher: &ServiceDaemon, browser: &ServiceDaemon) -> (Metrics, Metrics) {
    let publisher = metrics(publisher);
    let browser = metrics(browser);
    let mut report = serde_json::json!({
        "sample": label,
        "publisher_srv": publisher["cached-srv"],
        "publisher_txt": publisher["cached-txt"],
        "publisher_timers": publisher["timer"],
        "browser_srv": browser["cached-srv"],
        "browser_txt": browser["cached-txt"],
        "browser_timers": browser["timer"],
    });
    if cfg!(target_os = "macos") {
        let output = std::process::Command::new("vmmap")
            .args(["-summary", &std::process::id().to_string()])
            .output()
            .unwrap();
        assert!(output.status.success());
        let output = String::from_utf8(output.stdout).unwrap();
        report["footprint"] = output
            .lines()
            .find(|line| line.starts_with("Physical footprint:"))
            .unwrap()
            .trim()
            .into();
    }
    println!("{}", report);
    (publisher, browser)
}

fn start(port: u16) -> ServiceDaemon {
    let daemon = ServiceDaemon::new_with_port(port).unwrap();
    daemon.disable_interface(IfKind::All).unwrap();
    daemon.enable_interface(IfKind::LoopbackV4).unwrap();
    daemon.set_multicast_loop_v4(true).unwrap();
    daemon
}

fn main() {
    // Select an unused private port, then release it for the multicast sockets.
    let reservation = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = reservation.local_addr().unwrap().port();
    drop(reservation);
    let publisher = start(port);
    let browser = start(port);
    // Exercise the cache even without PTRs, as a cache-only browser can do.
    browser.accept_unsolicited(true).unwrap();
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).unwrap();
    socket.set_multicast_if_v4(&Ipv4Addr::LOCALHOST).unwrap();
    socket.set_multicast_loop_v4(true).unwrap();
    let destination = SocketAddrV4::new(Ipv4Addr::new(224, 0, 0, 251), port);
    sleep(Duration::from_secs(1));
    sample("baseline", &publisher, &browser);
    for round in 0..5 {
        for i in 0..2_000 {
            let owner = format!("churn-{round}-{i}._memory._tcp.local.");
            let mut packet = vec![0, 0, 0x84, 0, 0, 0, 0, 2, 0, 0, 0, 0];
            let mut srv = vec![0, 0, 0, 0, 0, 80];
            srv.extend(name("host.local."));
            record(&mut packet, &owner, 33, &srv);
            record(&mut packet, &owner, 16, b"\x03a=b");
            socket.send_to(&packet, &destination.into()).unwrap();
            sleep(Duration::from_micros(500));
        }
        let (received_publisher, received_browser) =
            sample(&format!("round-{round}-received"), &publisher, &browser);
        assert!(
            received_browser["cached-srv"] > 0,
            "fixture received no records"
        );
        for key in ["cached-srv", "cached-txt"] {
            assert_eq!(received_publisher[key], 0, "publisher retained {key}");
        }
        sleep(Duration::from_secs(3));
        sample(&format!("round-{round}-expired"), &publisher, &browser);
        for key in ["cached-srv", "cached-txt"] {
            assert_eq!(metrics(&browser)[key], 0, "expired {key} remain");
        }
        assert!(metrics(&publisher)["timer"] <= 1);
        assert!(metrics(&browser)["timer"] <= 1);
    }
    for daemon in [&publisher, &browser] {
        daemon
            .shutdown()
            .unwrap()
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
    }
}
