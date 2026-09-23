//! Signed, unprivileged updates. The installed executable remains the launcher;
//! only serve commands load verified releases from the relay's private state.
use anyhow::{Context, Result, ensure};
use ed25519_dalek::{Signature, VerifyingKey};
use removent_relay::config::ServerConfig;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::Duration,
};

const PUBLIC_KEY: &str = "2b51160721f9eee916944e4603e6540c628247e5284a79b099392f086a3def0c";
const RELEASES: &str = "https://github.com/backrunner/removent/releases";
const MAX_BINARY: u64 = 128 * 1024 * 1024;
const MAX_MANIFEST: u64 = 16384;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: String,
    platform: String,
    url: String,
    sha256: String,
    binary_sha256: String,
    signature: String,
}

fn platform() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64" | "x86_64") => Ok("macos-universal"),
        ("linux", "x86_64") => Ok("linux-x86_64"),
        ("linux", "aarch64") => Ok("linux-aarch64"),
        _ => anyhow::bail!("No relay update package for this platform"),
    }
}

fn version(text: &str) -> Result<semver::Version> {
    let value = semver::Version::parse(text).context("Invalid relay release version")?;
    ensure!(
        value.pre.is_empty() && value.build.is_empty(),
        "Expected a stable relay release"
    );
    Ok(value)
}

fn current_version() -> semver::Version {
    // The installed build may be a beta; downloaded releases must be stable.
    semver::Version::parse(env!("CARGO_PKG_VERSION")).expect("Cargo supplies a valid SemVer")
}

impl Manifest {
    fn payload(&self) -> String {
        format!(
            "removent-relay-v1\n{}\n{}\n{}\n{}\n{}",
            self.version, self.platform, self.url, self.sha256, self.binary_sha256
        )
    }

    fn verify(&self, key: &VerifyingKey) -> Result<()> {
        version(&self.version)?;
        ensure!(
            self.platform == platform()?,
            "Relay update targets another platform"
        );
        let expected = format!(
            "{RELEASES}/download/v{0}/removent-relay-v{0}-{1}.tar.gz",
            self.version, self.platform
        );
        ensure!(self.url == expected, "Unexpected relay release URL");
        for hash in [&self.sha256, &self.binary_sha256] {
            ensure!(
                hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()),
                "Invalid relay release hash"
            );
        }
        let bytes = hex::decode(&self.signature).context("Invalid relay release signature")?;
        let signature = Signature::from_slice(&bytes).context("Invalid relay release signature")?;
        key.verify_strict(self.payload().as_bytes(), &signature)
            .context("Relay release signature verification failed")
    }

    fn binary(&self, root: &Path) -> PathBuf {
        root.join(format!("v{}", self.version))
            .join("removent-relay")
    }
}

fn key() -> VerifyingKey {
    VerifyingKey::from_bytes(&hex::decode(PUBLIC_KEY).unwrap().try_into().unwrap()).unwrap()
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::limited(5))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(180))
        .user_agent(concat!("removent-relay/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

async fn download(
    client: &reqwest::Client,
    url: &str,
    limit: u64,
    output: &mut impl Write,
) -> Result<()> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    ensure!(
        response.content_length().is_none_or(|n| n <= limit),
        "Relay release download is too large"
    );
    let mut size = 0;
    while let Some(chunk) = response.chunk().await? {
        size += chunk.len() as u64;
        ensure!(size <= limit, "Relay release download is too large");
        output.write_all(&chunk)?;
    }
    Ok(())
}

async fn latest(client: &reqwest::Client) -> Result<Manifest> {
    let mut bytes = Vec::new();
    download(
        client,
        &format!(
            "{RELEASES}/latest/download/relay-latest-{}.json",
            platform()?
        ),
        MAX_MANIFEST,
        &mut bytes,
    )
    .await?;
    let manifest: Manifest =
        serde_json::from_slice(&bytes).context("Invalid relay update manifest")?;
    manifest.verify(&key())?;
    Ok(manifest)
}

fn open_regular(path: &Path, limit: u64) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    ensure!(
        meta.is_file() && meta.len() <= limit,
        "Invalid relay update file"
    );
    Ok(file)
}

fn hash_file(path: &Path) -> Result<String> {
    let mut reader = open_regular(path, MAX_BINARY)?.take(MAX_BINARY + 1);
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    let mut size = 0;
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        size += n as u64;
        ensure!(size <= MAX_BINARY, "Relay update file is too large");
        hash.update(&buffer[..n]);
    }
    Ok(hex::encode(hash.finalize()))
}

fn root(config: &ServerConfig) -> PathBuf {
    config.identity_dir.join("updates")
}

fn active(config: &ServerConfig) -> Result<Option<Manifest>> {
    read_active(config, &key())
}

fn read_active(config: &ServerConfig, key: &VerifyingKey) -> Result<Option<Manifest>> {
    let path = root(config).join("active.json");
    let file = match open_regular(&path, MAX_MANIFEST) {
        Ok(file) => file,
        Err(e)
            if e.downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok(None);
        }
        Err(e) => return Err(e),
    };
    let manifest: Manifest = serde_json::from_reader(file.take(MAX_MANIFEST + 1))?;
    manifest.verify(key)?;
    ensure!(
        hash_file(&manifest.binary(&root(config)))? == manifest.binary_sha256,
        "Installed relay update checksum mismatch"
    );
    Ok(Some(manifest))
}

/// Called only for native serving, never CLI management or containers. Even a
/// disabled scheduler continues to use the last installed version.
pub fn installed(config: &ServerConfig) -> Result<Option<PathBuf>> {
    let root = root(config);
    if root.try_exists()? {
        private_directory(&root)?;
    }
    Ok(active(config)?
        .filter(|m| version(&m.version).unwrap() > current_version())
        .map(|m| m.binary(&root)))
}

fn private_directory(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    ensure!(
        meta.is_dir() && meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o077 == 0,
        "Relay updates must use a private directory owned by the service user"
    );
    Ok(())
}

pub async fn check_command(config: Option<&ServerConfig>) -> Result<()> {
    let installed = config.map(active).transpose()?.flatten();
    let current = installed
        .as_ref()
        .map(|m| m.version.as_str())
        .filter(|v| version(v).unwrap() > current_version())
        .unwrap_or(env!("CARGO_PKG_VERSION"));
    println!(
        "{} version: {current}",
        if config.is_some() { "Service" } else { "CLI" }
    );
    let manifest = latest(&client()?).await?;
    if version(&manifest.version)? > semver::Version::parse(current)? {
        println!(
            "Update available: {}\nAutomatic installation: enable [updates] enabled = true in server.toml, then restart the relay.",
            manifest.version
        );
    } else {
        println!("Relay is up to date (latest stable: {}).", manifest.version);
    }
    Ok(())
}

fn unpack(archive: &Path, output: &Path, manifest: &Manifest) -> Result<()> {
    ensure!(
        hash_file(archive)? == manifest.sha256,
        "Relay archive checksum mismatch"
    );
    let reader = flate2::read::GzDecoder::new(open_regular(archive, MAX_BINARY)?);
    let mut tar = tar::Archive::new(reader.take(MAX_BINARY + 65536));
    let mut entries = tar.entries()?;
    let mut entry = entries.next().context("Empty relay archive")??;
    ensure!(
        entry.path()? == Path::new("removent-relay")
            && entry.header().entry_type().is_file()
            && entry.size() <= MAX_BINARY,
        "Unexpected relay archive contents"
    );
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o700)
        .open(output)?;
    std::io::copy(&mut entry, &mut file)?;
    file.sync_all()?;
    drop(entry);
    ensure!(
        entries.next().is_none(),
        "Unexpected extra relay archive entry"
    );
    ensure!(
        hash_file(output)? == manifest.binary_sha256,
        "Relay binary checksum mismatch"
    );
    Ok(())
}

async fn probe(binary: &Path, manifest: &Manifest, config: &ServerConfig) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let status = tokio::time::timeout(Duration::from_secs(30), tokio::process::Command::new("/usr/bin/codesign")
            .args(["--verify", "--all-architectures", "--strict", "-R",
                "=anchor apple generic and certificate leaf[subject.OU] = \"PB8H83VL3Z\" and certificate leaf[field.1.2.840.113635.100.6.1.13] exists"])
            .arg(binary).kill_on_drop(true).output()).await??;
        ensure!(
            status.status.success(),
            "Relay Developer ID verification failed"
        );
    }
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        tokio::process::Command::new(binary)
            .arg("--version")
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    ensure!(
        result.status.success()
            && String::from_utf8_lossy(&result.stdout).trim()
                == format!("removent-relay {}", manifest.version),
        "Downloaded relay does not report the release version"
    );
    // The new parser must accept this configuration before it can replace us.
    let mut file = tempfile::NamedTempFile::new_in(binary.parent().unwrap())?;
    file.write_all(toml::to_string(config)?.as_bytes())?;
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        tokio::process::Command::new(binary)
            .arg("check")
            .arg(file.path())
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    ensure!(
        result.status.success(),
        "New relay cannot read the current configuration"
    );
    Ok(())
}

async fn install(
    client: &reqwest::Client,
    manifest: &Manifest,
    config: &ServerConfig,
) -> Result<PathBuf> {
    let root = root(config);
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&root)?;
    private_directory(&root)?;
    let _lock = removent_core::DataDirLock::acquire_at(&root.join(".update.lock"))?;
    let stage = tempfile::Builder::new()
        .prefix(".stage-")
        .tempdir_in(&root)?;
    let archive = stage.path().join("release.tar.gz");
    let mut file = File::create(&archive)?;
    download(client, &manifest.url, MAX_BINARY, &mut file).await?;
    file.sync_all()?;
    let binary = stage.path().join("removent-relay");
    unpack(&archive, &binary, manifest)?;
    fs::remove_file(archive)?;
    probe(&binary, manifest, config).await?;
    let destination = manifest.binary(&root);
    let directory = destination.parent().unwrap();
    // Reuse a complete previous download; never replace an executing inode.
    if directory.exists() {
        ensure!(
            hash_file(&destination)? == manifest.binary_sha256,
            "Conflicting staged relay release"
        );
    } else {
        fs::rename(stage.path(), directory)?;
    }
    activate(&root, manifest)?;
    // Keep the current and next releases only. Temporary/incomplete downloads
    // are managed by TempDir, and configuration/identity never enter this tree.
    // Retention failure must not turn a completed installation into a retry.
    for entry in fs::read_dir(&root).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if entry.file_type().is_ok_and(|kind| kind.is_dir())
            && name.strip_prefix('v').is_some_and(|v| {
                version(v).is_ok() && v != manifest.version && v != env!("CARGO_PKG_VERSION")
            })
        {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
    Ok(destination)
}

fn activate(root: &Path, manifest: &Manifest) -> Result<()> {
    let mut pointer = tempfile::NamedTempFile::new_in(root)?;
    pointer.write_all(&serde_json::to_vec(manifest)?)?;
    pointer.as_file().sync_all()?;
    if root.join("active.json").exists() {
        let mut previous = tempfile::NamedTempFile::new_in(root)?;
        std::io::copy(
            &mut open_regular(&root.join("active.json"), MAX_MANIFEST)?,
            &mut previous,
        )?;
        previous.as_file().sync_all()?;
        previous.persist(root.join("previous.json"))?;
    }
    pointer.persist(root.join("active.json"))?;
    File::open(root)?.sync_all()?;
    Ok(())
}

pub fn rollback(binary: &Path) -> Result<()> {
    let root = binary
        .parent()
        .and_then(Path::parent)
        .context("Invalid update path")?;
    let previous = root.join("previous.json");
    if previous.exists() {
        fs::rename(previous, root.join("active.json"))?;
    } else {
        fs::remove_file(root.join("active.json"))?;
    }
    File::open(root)?.sync_all()?;
    Ok(())
}

/// Disabled means no timer and no network. A failed check/install leaves the
/// current relay serving and retries only at the configured interval.
pub async fn schedule(config: ServerConfig) -> PathBuf {
    schedule_with(config, check_once).await
}

async fn check_once(config: ServerConfig) -> Result<Option<PathBuf>> {
    let client = client()?;
    let manifest = latest(&client).await?;
    if version(&manifest.version)? <= current_version() {
        return Ok(None);
    }
    tracing::info!(version = %manifest.version, "Installing signed relay update");
    install(&client, &manifest, &config).await.map(Some)
}

async fn schedule_with<F, Fut>(config: ServerConfig, mut check: F) -> PathBuf
where
    F: FnMut(ServerConfig) -> Fut,
    Fut: std::future::Future<Output = Result<Option<PathBuf>>>,
{
    if !config.updates.enabled {
        return std::future::pending().await;
    }
    tokio::time::sleep(Duration::from_secs(30)).await;
    loop {
        let result = check(config.clone()).await;
        match result {
            Ok(Some(path)) => return path,
            Ok(None) => tracing::info!("Relay is up to date"),
            Err(error) => tracing::warn!(%error, "Relay update failed; current service retained"),
        }
        tokio::time::sleep(Duration::from_secs(config.updates.check_interval_secs)).await;
    }
}

#[cfg(test)]
mod tests {
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
}
