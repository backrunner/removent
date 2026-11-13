//! Actual QUIC host/client session using the isolated desktop fixture.
use removent_core::{
    AuthenticationMode, AuthenticationSettings, DataPaths, MemoryClipboard, Settings, TextClipboard,
};
use removent_proto::{Caps, CodecId, KeyKind, KeyModifiers, MouseKind};
use std::{
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    #[cfg(windows)]
    removent_input::initialize_host_dpi();
    anyhow::ensure!(
        std::env::var("REMOVENT_DESKTOP_TEST").as_deref() == Ok("1"),
        "Requires isolated desktop"
    );
    tracing_subscriber::fmt().with_env_filter("info").init();
    tokio::time::timeout(Duration::from_secs(35), probe()).await??;
    Ok(())
}
async fn probe() -> anyhow::Result<()> {
    let host_dir = tempfile::tempdir()?;
    let client_dir = tempfile::tempdir()?;
    let hp = DataPaths {
        root: host_dir.path().into(),
    };
    let cp = DataPaths {
        root: client_dir.path().into(),
    };
    let password = hex::encode(rand::random::<[u8; 32]>());
    let port = std::net::UdpSocket::bind("127.0.0.1:0")?
        .local_addr()?
        .port();
    let settings = Settings {
        host_enabled: true,
        host_port: port,
        authentication: AuthenticationSettings {
            mode: AuthenticationMode::Password,
            password: password.clone(),
            ..Default::default()
        },
        ..Default::default()
    };
    settings.save(&hp)?;
    let host_id = removent_core::identity::load_or_create(&hp, "Isolated host")?;
    let client_id = removent_core::identity::load_or_create(&cp, "Isolated controller")?;
    let (ready, mut readiness) = tokio::sync::mpsc::channel(1);
    let cancel = tokio_util::sync::CancellationToken::new();
    let _guard = cancel.clone().drop_guard();
    struct Clipboard;
    impl TextClipboard for Clipboard {
        fn change_count(&self) -> Result<u64, String> {
            removent_input::change_count().map_err(|e| e.to_string())
        }
        fn read(&self) -> Result<String, String> {
            removent_input::read_text().map_err(|e| e.to_string())
        }
        fn write(&self, text: &str) -> Result<(), String> {
            removent_input::write_text(text).map_err(|e| e.to_string())
        }
    }
    let host = tokio::spawn(removent_host::serve_forever(
        removent_host::HostRunnerConfig {
            paths: hp,
            settings,
            input_sink: Some(Arc::new(removent_host::RealInputSink::new())),
            local_clip: Some(Arc::new(Clipboard)),
        },
        removent_host::HostCallbacks {
            show_pairing_pin: Box::new(|_| {}),
            admission_prompt: Box::new(|_, _| Box::pin(async { true })),
            on_event: Box::new(move |event| {
                if matches!(event, removent_host::HostEvent::Listening) {
                    let _ = ready.try_send(());
                }
            }),
        },
        Arc::new(AtomicBool::new(true)),
        cancel.clone(),
    ));
    tokio::time::timeout(Duration::from_secs(5), readiness.recv())
        .await?
        .ok_or_else(|| anyhow::anyhow!("Host failed before listening"))?;
    let (endpoint, _) = removent_net::make_client_endpoint(
        "127.0.0.1:0".parse()?,
        &client_id,
        removent_net::PinState::new([host_id.fingerprint], false),
    )?;
    let clipboard = MemoryClipboard::new();
    clipboard
        .write("Removent QUIC clipboard")
        .map_err(anyhow::Error::msg)?;
    let mut session = removent_client::connect_session(
        endpoint,
        std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        &client_id,
        removent_client::ClientConfig {
            native_video: false,
            video_quality: Default::default(),
            host_identity_verified: true,
            pairing_code: None,
            device_name: "Controller".into(),
            caps: Caps::all(),
            local_clip: Some(clipboard.clone()),
        },
        None,
        None,
        Some(Box::new(move |_, send| {
            let _ = send.send(password);
        })),
    )
    .await?;
    anyhow::ensure!(
        session.negotiated.video.codec == CodecId::H264,
        "Portable session must negotiate H264"
    );
    anyhow::ensure!(
        !session.negotiated.audio.enabled,
        "Portable host must not advertise system audio"
    );
    let frame = tokio::time::timeout(Duration::from_secs(10), session.decoded_bgra_rx.recv())
        .await?
        .ok_or_else(|| anyhow::anyhow!("No network video frame"))?;
    anyhow::ensure!(
        frame
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| p[2] > 160 && p[1] < 100 && p[0] < 100),
        "Network frame lacks fixture pixels"
    );
    let display =
        removent_input::DisplayGeometry::main().ok_or_else(|| anyhow::anyhow!("No display"))?;
    let x = 200. * frame.width as f32 / display.width_px as f32;
    let y = 200. * frame.height as f32 / display.height_px as f32;
    session
        .send_input_mouse(display.id as u64, x, y, 1, MouseKind::LeftDown)
        .await?;
    session
        .send_input_mouse(display.id as u64, x, y, 0, MouseKind::LeftUp)
        .await?;
    for (vk, mods, kind) in [
        (0x3b, KeyModifiers::CONTROL, KeyKind::FlagsChanged),
        (0x00, KeyModifiers::CONTROL, KeyKind::Down),
        (0x00, KeyModifiers::CONTROL, KeyKind::Up),
        (0x3b, KeyModifiers::empty(), KeyKind::FlagsChanged),
    ] {
        session.send_input_key(vk, mods, kind, None).await?;
    }
    for ch in "hello".chars() {
        session
            .send_input_key(
                removent_input::UNICODE_ONLY_VK,
                KeyModifiers::empty(),
                KeyKind::Down,
                Some(ch),
            )
            .await?;
        session
            .send_input_key(
                removent_input::UNICODE_ONLY_VK,
                KeyModifiers::empty(),
                KeyKind::Up,
                Some(ch),
            )
            .await?;
    }
    clipboard
        .write("Controller to native host")
        .map_err(anyhow::Error::msg)?;
    tokio::time::timeout(Duration::from_secs(5), async {
        while removent_input::read_text().ok().as_deref() != Some("Controller to native host") {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await?;
    removent_input::write_text("Native host to controller")?;
    tokio::time::timeout(Duration::from_secs(5), async {
        while clipboard.read().ok().as_deref() != Some("Native host to controller") {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await?;
    tokio::time::sleep(Duration::from_millis(250)).await;
    session.close().await?;
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), host).await???;
    println!(
        "PASS: authenticated QUIC session, negotiated H264, real desktop pixels, remote input, bidirectional native clipboard and clean teardown"
    );
    Ok(())
}
