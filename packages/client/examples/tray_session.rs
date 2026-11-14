//! Real remote sessions used by native tray admission and lifetime acceptance.
use removent_core::{DataPaths, PeerRecord, PeersStore};
use removent_proto::Caps;
use std::{
    io::{BufRead, Write},
    time::Duration,
};

fn emit(value: serde_json::Value) {
    println!("{value}");
    std::io::stdout().flush().unwrap();
}
fn commands() -> tokio::sync::mpsc::UnboundedReceiver<String> {
    let (send, receive) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if send.send(line).is_err() {
                break;
            }
        }
    });
    receive
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    anyhow::ensure!(
        std::env::var("REMOVENT_DESKTOP_TEST").as_deref() == Ok("1"),
        "Requires isolated desktop"
    );
    let args: Vec<_> = std::env::args().collect();
    let hp = DataPaths::resolve();
    let cp = DataPaths {
        root: std::env::var_os("REMOVENT_TEST_CLIENT_DIR")
            .ok_or_else(|| anyhow::anyhow!("Missing isolated client directory"))?
            .into(),
    };
    let client = removent_core::identity::load_or_create(&cp, "Tray acceptance 中文")?;
    let settings = removent_core::Settings::load_strict(&hp)?;
    let host = removent_core::identity::load_or_create(&hp, &settings.device_name)?;
    if args.get(1).is_some_and(|arg| arg == "prepare") {
        PeersStore::load(&hp)?.upsert(PeerRecord {
            fingerprint: client.fingerprint_hex(),
            name: "Tray acceptance 中文".into(),
            short_fp: client.short_fingerprint_hex(),
            trusted: true,
            granted_caps: Caps::none(),
            added_at_unix: 0,
            last_connected_unix: 0,
        })?;
        return Ok(());
    }
    let port: u16 = args
        .get(1)
        .ok_or_else(|| anyhow::anyhow!("Missing host port"))?
        .parse()?;
    let (endpoint, _) = removent_net::make_client_endpoint(
        "127.0.0.1:0".parse()?,
        &client,
        removent_net::PinState::new([host.fingerprint], false),
    )?;
    let mut commands = commands();
    let connect = removent_client::connect_session(
        endpoint.clone(),
        ([127, 0, 0, 1], port).into(),
        &client,
        removent_client::ClientConfig {
            native_video: false,
            video_quality: Default::default(),
            host_identity_verified: true,
            pairing_code: None,
            device_name: "Tray acceptance 中文".into(),
            caps: Caps {
                video: true,
                ..Caps::none()
            },
            local_clip: None,
        },
        None,
        None,
        None,
    );
    let result = tokio::select! {
        result = tokio::time::timeout(Duration::from_secs(45), connect) => result?,
        _ = commands.recv() => {
            endpoint.close(0u32.into(), b"tray QA cancellation");
            endpoint.wait_idle().await;
            emit(serde_json::json!({"event":"cancelled"}));
            return Ok(());
        }
    };
    let mut session = match result {
        Ok(session) => session,
        Err(error) => {
            emit(serde_json::json!({"event":"rejected","reason":error.to_string()}));
            return Ok(());
        }
    };
    emit(serde_json::json!({"event":"connected"}));
    let mut frames = 0;
    let mut report = tokio::time::interval(Duration::from_secs(1));
    let deadline = tokio::time::sleep(Duration::from_secs(1200));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            frame = session.decoded_bgra_rx.recv() => {
                anyhow::ensure!(frame.is_some(), "Remote video stopped");
                frames += 1;
            },
            _ = report.tick() => emit(serde_json::json!({"event":"frames","count":frames})),
            _ = commands.recv() => break,
            _ = &mut deadline => anyhow::bail!("Tray session fixture timed out"),
        }
    }
    session.close().await?;
    emit(serde_json::json!({"event":"closed","frames":frames}));
    Ok(())
}
