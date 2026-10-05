//! Synthetic loopback host for iOS simulator acceptance. Never captures a screen
//! or injects real input. The PIN/status endpoint is bound to loopback only.
use removent_core::{DataPaths, MemoryClipboard, PeersStore, TextClipboard};
use removent_host::{HostConfig, HostInteractions, RecorderInputSink};
use removent_net::{PinState, RvpConnection};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let paths = DataPaths {
        root: dir.path().into(),
    };
    let identity = removent_core::identity::load_or_create(&paths, "Mobile Test Computer")?;
    let proactive = std::env::var("REMOVENT_FIXTURE_INVITATION").as_deref() == Ok("1");
    let invitation = if proactive {
        Some(removent_core::pairing_invitation::Invitation::generate(
            &paths,
        )?)
    } else {
        None
    };
    let advertisement = if proactive {
        let ad = removent_net::Advertiser::start(
            "Mobile Test Computer",
            &identity.short_fingerprint_hex(),
            48689,
            removent_proto::Caps::all(),
        )?;
        ad.set_pairing(invitation.as_ref().map(|i| i.advertisement()))?;
        Some(ad)
    } else {
        None
    };
    let _keep_advertisement = advertisement;
    let (endpoint, _) = removent_net::make_server_endpoint(
        if proactive {
            "0.0.0.0:48689"
        } else {
            "127.0.0.1:48689"
        }
        .parse()?,
        &identity,
        PinState::new([], true),
    )?;
    // Optional local QUIC relay verifies the two consecutive mobile trust prompts.
    if std::env::var("REMOVENT_FIXTURE_RELAY").as_deref() == Ok("1") {
        use removent_relay::config::{Room, ServerConfig, TunnelConfig, token_hash};
        let relay_paths = DataPaths {
            root: paths.root.join("relay"),
        };
        let relay = removent_core::identity::load_or_create(&relay_paths, "Mobile Test Relay")?;
        let address = "127.0.0.1:48691".parse()?;
        let relay_endpoint = removent_relay::server_endpoint(address, &relay)?;
        let host_token = "11".repeat(32);
        let config = ServerConfig {
            updates: Default::default(),
            listen: address,
            allowed_cidrs: vec![],
            identity_dir: relay_paths.root,
            max_connections: 16,
            max_clients_per_room: 4,
            max_bytes_per_second: 1_000_000_000,
            rooms: vec![Room {
                name: "office".into(),
                host_token_sha256: hex::encode(token_hash(&host_token)?),
                client_token_sha256: hex::encode(token_hash(&"22".repeat(32))?),
                host_public_keys: vec![],
                client_public_keys: vec![],
            }],
        };
        tokio::spawn(removent_relay::server::serve(
            relay_endpoint,
            config,
            tokio_util::sync::CancellationToken::new(),
        ));
        let tunnel = TunnelConfig {
            server: format!("removent://{address}"),
            transport: removent_core::removent_uri::RelayTransport::Quic,
            insecure_loopback: false,
            server_name: String::new(),
            accept_invalid_certificate: false,
            server_fingerprint: relay.fingerprint_hex(),
            host_fingerprint: identity.fingerprint_hex(),
            room: "office".into(),
            token: host_token,
        };
        let target = endpoint.local_addr()?;
        let host_identity = identity.clone();
        tokio::spawn(async move {
            loop {
                if let Ok(tunnel) =
                    removent_relay::client::connect_host(&tunnel, &host_identity).await
                {
                    let _ = tunnel.run(target).await;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        });
    }
    let pin = Arc::new(Mutex::new(String::new()));
    let recorder = Arc::new(RecorderInputSink::default());
    let clipboard = MemoryClipboard::new();
    clipboard
        .write("Removent mobile clipboard fixture")
        .map_err(anyhow::Error::msg)?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:48690").await?;
    let status_pin = pin.clone();
    let status_input = recorder.clone();
    let status_clip = clipboard.clone();
    let status_paths = paths.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut request = [0; 1024];
            let _ = stream.read(&mut request).await;
            let auth_mode =
                std::env::var("REMOVENT_FIXTURE_AUTH").unwrap_or_else(|_| "pairing_code".into());
            let auth = removent_core::AuthenticationSettings {
                mode: removent_core::AuthenticationMode::Otp,
                otp_secret: "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ".into(),
                ..Default::default()
            };
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let typed_text: String = status_input
                .events
                .lock()
                .unwrap()
                .iter()
                .filter_map(|event| match event {
                    removent_host::RecordedInput::Key {
                        kind: removent_proto::KeyKind::Down,
                        unicode: Some(ch),
                        ..
                    } => Some(*ch),
                    _ => None,
                })
                .collect();
            let keys: Vec<_> = status_input.events.lock().unwrap().iter().filter_map(|event| {
                match event {
                    removent_host::RecordedInput::Key { vk, mods, kind, unicode: None } => Some(serde_json::json!({
                        "code":vk, "modifiers":mods.bits(), "down":*kind == removent_proto::KeyKind::Down
                    })),
                    _ => None,
                }
            }).collect();
            let body = serde_json::json!({"relay":std::env::var("REMOVENT_FIXTURE_RELAY").as_deref() == Ok("1"), "auth_mode":auth_mode, "otp":auth.totp().unwrap().generate(now), "pin":*status_pin.lock().unwrap(),
                "inputs":status_input.events.lock().unwrap().len(),
                "typed_text":typed_text,
                "keys":keys,
                "clipboard":status_clip.read().unwrap_or_default(), "invitation":removent_core::pairing_invitation::Invitation::load(&status_paths).ok().flatten().map(|i| i.code().expose().to_owned())})
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });
    println!("Synthetic RVP host: 127.0.0.1:48689; status: http://127.0.0.1:48690");
    let mut peers = PeersStore::in_memory();
    while let Some(incoming) = endpoint.accept().await {
        let conn = match incoming.await {
            Ok(c) => RvpConnection::new(c),
            Err(e) => {
                eprintln!("{e}");
                continue;
            }
        };
        let pin = pin.clone();
        let cfg = HostConfig {
            authentication: match std::env::var("REMOVENT_FIXTURE_AUTH").as_deref() {
                Ok("password") => removent_core::AuthenticationSettings {
                    mode: removent_core::AuthenticationMode::Password,
                    password: "fixture-password".into(),
                    ..Default::default()
                },
                Ok("otp") => removent_core::AuthenticationSettings {
                    mode: removent_core::AuthenticationMode::Otp,
                    otp_secret: "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ".into(),
                    ..Default::default()
                },
                Ok("none") => removent_core::AuthenticationSettings {
                    mode: removent_core::AuthenticationMode::None,
                    ..Default::default()
                },
                _ => Default::default(),
            },
            auth_paths: Some(paths.clone()),
            device_name: "Mobile Test Computer".into(),
            audio_available: true,
            preapproved_only: false,
            admission: removent_core::AdmissionMode::AlwaysAsk,
            video_bitrate_kbps: 3000,
            video_fps: 30,
            input_sink: Some(recorder.clone()),
            local_clip: Some(clipboard.clone()),
        };
        let result = removent_host::serve_connection(
            conn.clone(),
            &identity,
            &mut peers,
            &cfg,
            HostInteractions {
                show_pairing_pin: Box::new(move |value| {
                    *pin.lock().unwrap() = value;
                }),
                admission_prompt: Box::new(|_, _| Box::pin(async { true })),
            },
            removent_proto::DisplayInfo {
                id: 1,
                w_px: 640,
                h_px: 360,
                scale: 1.,
                dpi: 96,
                is_main: true,
            },
        )
        .await;
        let (session, kf, quality, commands, sink, source) = match result {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{e}");
                continue;
            }
        };
        let deps = removent_host::ControlPumpDeps {
            conn: conn.clone(),
            kf_tx: None,
            controller: None,
            window_ms: 250,
            input: Some(recorder.clone()),
            local_clip: Some(clipboard.clone()),
            quality_tx: None,
            caps: session.peer_caps,
            clip_state: session.clip_state.clone(),
            cancel: session.cancel.clone(),
            peer_fp: Some(session.peer_fp_hex.clone()),
            delivery: None,
        };
        let pump = removent_host::spawn_control_pump(source, sink, deps, commands);
        let (tx, rx) = removent_core::latest::channel();
        let video = removent_host::spawn_video_loop(
            conn.open_media_stream().await?,
            rx,
            kf,
            quality,
            session.ack.video.codec,
            640,
            360,
            3000,
            30,
            session.cancel.clone(),
            None,
            0,
            None,
            None,
        );
        let (atx, arx) = tokio::sync::mpsc::channel(8);
        let audio = if session.ack.audio.enabled {
            Some(removent_host::spawn_audio_loop(
                conn.open_media_stream().await?,
                arx,
                64,
                session.cancel.clone(),
            ))
        } else {
            None
        };
        let mut tick = tokio::time::interval(Duration::from_millis(10));
        let mut index = 0u64;
        loop {
            tokio::select! { _ = conn.inner().closed() => break, _ = tick.tick() => {} }
            if index.is_multiple_of(3) {
                let mut frame = vec![0u8; 640 * 360 * 4];
                for y in 0..360 {
                    for x in 0..640 {
                        let offset = (y * 640 + x) * 4;
                        frame[offset] = (x * 255 / 640) as u8;
                        frame[offset + 1] = (y * 255 / 360) as u8;
                        frame[offset + 2] = if x / 40 % 2 == y / 40 % 2 { 220 } else { 40 };
                        frame[offset + 3] = 255;
                    }
                }
                let x = (index as usize * 3) % 620;
                for y in 160..180 {
                    for xx in x..x + 20 {
                        frame[(y * 640 + xx) * 4..(y * 640 + xx) * 4 + 4]
                            .copy_from_slice(&[255, 255, 255, 255]);
                    }
                }
                if tx.send((frame, (index * 10000) as i64)).is_err() {
                    break;
                }
            }
            if audio.is_some() {
                let samples = (0..480)
                    .flat_map(|n| {
                        let phase =
                            (index * 480 + n) as f64 * 440. * std::f64::consts::TAU / 48000.;
                        let value = (phase.sin() * 800.) as i16;
                        [value, value]
                    })
                    .collect();
                let _ = atx.try_send(removent_media_capture::AudioFrame {
                    samples,
                    pts_micros: (index * 10000) as i64,
                });
            }
            index += 1;
        }
        video.abort();
        pump.abort();
        if let Some(audio) = audio {
            audio.abort();
        }
    }
    Ok(())
}
