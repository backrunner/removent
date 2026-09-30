use super::*;
use ed25519_dalek::{Signer, SigningKey};
use std::os::unix::fs::{PermissionsExt, symlink};

fn fixture() -> (Manifest, SigningKey) {
    let signer = SigningKey::from_bytes(&[42; 32]);
    let mut manifest = Manifest {
        version: "9.0.0".into(),
        platform: platform().unwrap().into(),
        url: format!(
            "{RELEASES}/download/v9.0.0/removent-relay-v9.0.0-{}.tar.gz",
            platform().unwrap()
        ),
        sha256: "ab".repeat(32),
        binary_sha256: hex::encode(Sha256::digest(b"binary")),
        signature: String::new(),
    };
    manifest.signature = hex::encode(signer.sign(manifest.payload().as_bytes()).to_bytes());
    (manifest, signer)
}

fn config(root: &Path) -> ServerConfig {
    ServerConfig {
        listen: "127.0.0.1:48700".parse().unwrap(),
        identity_dir: root.to_path_buf(),
        max_connections: 128,
        max_clients_per_room: 4,
        max_bytes_per_second: 50000000,
        allowed_cidrs: vec![],
        rooms: vec![],
        updates: Default::default(),
    }
}

#[test]
fn signed_manifest_rejects_tampering_wrong_key_and_unsafe_versions() {
    let (manifest, signer) = fixture();
    manifest.verify(&signer.verifying_key()).unwrap();
    assert!(manifest.verify(&key()).is_err());
    for field in [
        "version",
        "platform",
        "url",
        "sha256",
        "binary_sha256",
        "signature",
    ] {
        let mut value = serde_json::to_value(&manifest).unwrap();
        value[field] = "tampered".into();
        let tampered: Manifest = serde_json::from_value(value).unwrap();
        assert!(tampered.verify(&signer.verifying_key()).is_err(), "{field}");
    }
    for invalid in [
        "../9.0.0",
        "9.0.0-rc.1",
        "9.0.0+build",
        "v9.0.0",
        "09.0.0",
        "9.0",
    ] {
        assert!(version(invalid).is_err(), "{invalid}");
    }
    assert!(version("0.10.0").unwrap() > version("0.9.99").unwrap());
    assert_eq!(current_version().to_string(), env!("CARGO_PKG_VERSION"));
}

#[test]
fn cache_verification_and_rollback_preserve_previous_release() {
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    let root = root(&config);
    fs::create_dir_all(&root).unwrap();
    assert!(read_active(&config, &key()).unwrap().is_none());
    let (first, signer) = fixture();
    fs::create_dir_all(first.binary(&root).parent().unwrap()).unwrap();
    fs::write(first.binary(&root), b"binary").unwrap();
    activate(&root, &first).unwrap();
    assert_eq!(
        read_active(&config, &signer.verifying_key())
            .unwrap()
            .unwrap()
            .version,
        "9.0.0"
    );
    assert!(active(&config).is_err()); // A test key is never trusted in production.
    let mut second = first.clone();
    second.version = "9.0.1".into();
    second.url = second.url.replace("9.0.0", "9.0.1");
    second.signature = hex::encode(signer.sign(second.payload().as_bytes()).to_bytes());
    fs::create_dir_all(second.binary(&root).parent().unwrap()).unwrap();
    fs::write(second.binary(&root), b"binary").unwrap();
    activate(&root, &second).unwrap();
    assert_eq!(
        read_active(&config, &signer.verifying_key())
            .unwrap()
            .unwrap()
            .version,
        "9.0.1"
    );
    rollback(&second.binary(&root)).unwrap();
    assert_eq!(
        read_active(&config, &signer.verifying_key())
            .unwrap()
            .unwrap()
            .version,
        "9.0.0"
    );
    fs::write(first.binary(&root), b"corrupt").unwrap();
    assert!(read_active(&config, &signer.verifying_key()).is_err());
    fs::remove_file(first.binary(&root)).unwrap();
    let other = temp.path().join("other");
    fs::write(&other, b"binary").unwrap();
    symlink(&other, first.binary(&root)).unwrap();
    assert!(read_active(&config, &signer.verifying_key()).is_err());
}

fn archive(path: &Path, kind: tar::EntryType, extra: bool) {
    let gzip =
        flate2::write::GzEncoder::new(File::create(path).unwrap(), flate2::Compression::fast());
    let mut archive = tar::Builder::new(gzip);
    let mut header = tar::Header::new_gnu();
    header.set_size(6);
    header.set_mode(0o755);
    header.set_entry_type(kind);
    header.set_cksum();
    archive
        .append_data(&mut header, "removent-relay", &b"binary"[..])
        .unwrap();
    if extra {
        archive
            .append_data(&mut header, "extra", &b"binary"[..])
            .unwrap();
    }
    archive.into_inner().unwrap().finish().unwrap();
}

#[test]
fn archives_are_bounded_and_must_contain_only_the_verified_regular_binary() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("update.tar.gz");
    for (index, kind, extra) in [
        (0, tar::EntryType::Regular, false),
        (1, tar::EntryType::Symlink, false),
        (2, tar::EntryType::Regular, true),
    ] {
        archive(&path, kind, extra);
        let (mut manifest, _) = fixture();
        manifest.sha256 = hash_file(&path).unwrap();
        let output = temp.path().join(format!("out-{index}"));
        let result = unpack(&path, &output, &manifest);
        assert_eq!(result.is_ok(), index == 0);
        if index == 0 {
            assert_eq!(fs::read(&output).unwrap(), b"binary");
            assert_eq!(
                fs::metadata(&output).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    }
    archive(&path, tar::EntryType::Regular, false);
    let (mut manifest, _) = fixture();
    assert!(unpack(&path, &temp.path().join("bad-hash"), &manifest).is_err());
    manifest.sha256 = hash_file(&path).unwrap();
    manifest.binary_sha256 = "00".repeat(32);
    assert!(unpack(&path, &temp.path().join("bad-binary"), &manifest).is_err());
}

#[tokio::test(start_paused = true)]
async fn disabled_updates_do_not_create_state_or_complete() {
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    assert!(!config.updates.enabled);
    assert!(
        tokio::time::timeout(
            Duration::from_secs(3 * 86400),
            schedule_with(config, |_| async {
                panic!("Disabled updates must never contact the release server")
            })
        )
        .await
        .is_err()
    );
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[tokio::test(start_paused = true)]
async fn enabled_scheduler_retries_failures_at_the_configured_interval() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = config(temp.path());
    config.updates.enabled = true;
    config.updates.check_interval_secs = 7200;
    let started = tokio::time::Instant::now();
    let mut attempts = 0;
    let next = schedule_with(config, |_| {
        attempts += 1;
        let attempt = attempts;
        assert_eq!(
            started.elapsed(),
            Duration::from_secs(30 + (attempt - 1) * 7200)
        );
        async move {
            if attempt == 1 {
                anyhow::bail!("release server unavailable");
            }
            Ok(Some(PathBuf::from("verified-update")))
        }
    })
    .await;
    assert_eq!(next, PathBuf::from("verified-update"));
    assert_eq!(attempts, 2);
}

#[tokio::test]
async fn download_limits_and_http_errors_are_enforced() {
    use std::net::TcpListener;
    for response in [
        "HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n",
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n6\r\nabcdef\r\n0\r\n\r\n",
        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n",
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = [0; 4096];
            let _ = socket.read(&mut request);
            let _ = socket.write_all(response.as_bytes());
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        assert!(
            download(&client, &format!("http://{address}"), 4, &mut Vec::new())
                .await
                .is_err()
        );
        server.join().unwrap();
    }
    // The production client refuses HTTP, even on loopback.
    assert!(
        client()
            .unwrap()
            .get("http://127.0.0.1:1")
            .send()
            .await
            .is_err()
    );
}
