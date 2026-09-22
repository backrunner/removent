//! Exercise the actual shipped executable, its generated profiles and shutdown.
use removent_core::{DataPaths, identity};
use removent_relay::{
    client,
    config::{Role, TunnelConfig},
};
use std::{
    fs,
    net::UdpSocket,
    process::{Child, Command, Stdio},
    time::Duration,
};

const BINARY: &str = env!("CARGO_BIN_EXE_removent-relay");
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn generated_profiles_connect_to_the_binary_and_sigterm_preserves_identity() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("relay with spaces");
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap().to_string();
    drop(socket);
    let output = Command::new(BINARY)
        .args(["init", "--dir"])
        .arg(&dir)
        .args([
            "--address",
            &format!("removent://{address}"),
            "--listen",
            &address,
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let host_config = TunnelConfig::load(&dir.join("relay-host.toml")).unwrap();
    let client_config = TunnelConfig::load(&dir.join("relay-client.toml")).unwrap();
    assert!(!String::from_utf8_lossy(&output.stdout).contains(&host_config.token));
    assert!(!String::from_utf8_lossy(&output.stdout).contains(&client_config.token));
    let log_path = temp.path().join("relay.log");
    let log = fs::File::create(&log_path).unwrap();
    let mut process = Process(
        Command::new(BINARY)
            .arg("serve")
            .arg(dir.join("server.toml"))
            .env_remove("NOTIFY_SOCKET")
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap(),
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let log = fs::read_to_string(&log_path).unwrap();
            assert!(process.0.try_wait().unwrap().is_none(), "{log}");
            if log.contains("Relay listening") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let host = identity::load_or_create(
        &DataPaths {
            root: temp.path().join("host"),
        },
        "host",
    )
    .unwrap();
    let viewer = identity::load_or_create(
        &DataPaths {
            root: temp.path().join("viewer"),
        },
        "viewer",
    )
    .unwrap();
    let host_tunnel = client::connect(&host_config, Role::Host, &host)
        .await
        .unwrap();
    let viewer_tunnel = client::connect(&client_config, Role::Client, &viewer)
        .await
        .unwrap();
    assert!(
        client::connect(&client_config, Role::Host, &host)
            .await
            .is_err()
    );
    assert_eq!(
        unsafe { libc::kill(process.0.id() as i32, libc::SIGTERM) },
        0
    );
    let status = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(status) = process.0.try_wait().unwrap() {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(status.success());
    drop((host_tunnel, viewer_tunnel));
    let output = Command::new(BINARY)
        .args(["fingerprint", "--config"])
        .arg(dir.join("server.toml"))
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        host_config.server_fingerprint
    );
    assert_eq!(
        TunnelConfig::load(&dir.join("relay-host.toml"))
            .unwrap()
            .token,
        host_config.token
    );
}
