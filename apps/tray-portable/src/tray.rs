use removent_core::{
    DataDirLock, DataPaths,
    ipc::{IpcEvent, IpcRequest, IpcResponse, write_msg},
};
use rust_i18n::t;
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};
use tao::{
    event::Event,
    event_loop::{ControlFlow, EventLoopBuilder},
};
use tray_icon::{
    TrayIconBuilder,
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

fn sibling(name: &str) -> anyhow::Result<std::path::PathBuf> {
    let exe = std::env::current_exe()?;
    Ok(exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("No executable directory"))?
        .join(format!("{name}{}", std::env::consts::EXE_SUFFIX)))
}
fn launch(name: &str, paths: &DataPaths) -> anyhow::Result<()> {
    let mut command = std::process::Command::new(sibling(name)?);
    if name == "removentd" {
        command.arg("--background");
    }
    let mut child = command
        .env("REMOVENT_DATA_DIR", &paths.root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

pub fn run() -> anyhow::Result<()> {
    if std::env::args().any(|a| a == "--version") {
        println!("RemoventTray {}", removent_core::APP_VERSION);
        return Ok(());
    }
    let paths = DataPaths::resolve();
    paths.ensure_layout()?;
    let _lock = match DataDirLock::acquire_at(&paths.run_dir().join("tray.lock")) {
        Ok(lock) => lock,
        Err(removent_core::CoreError::AlreadyRunning) => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    let settings = removent_core::Settings::load_strict(&paths)?;
    rust_i18n::set_locale(removent_core::resolve_locale(settings.language));
    let event_loop = EventLoopBuilder::<()>::with_user_event().build();
    let wake = event_loop.create_proxy();
    // Tray-only processes have no windows; drive IPC/menu updates explicitly.
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(150));
            if wake.send_event(()).is_err() {
                break;
            }
        }
    });
    let menu = Menu::new();
    let status = MenuItem::new(t!("tray.connecting"), false, None);
    let open = MenuItem::new(t!("tray.open"), true, None);
    let sharing = CheckMenuItem::new(t!("tray.share"), false, false, None);
    let start = MenuItem::new(t!("tray.start"), true, None);
    let pair = MenuItem::new(t!("tray.pair"), false, None);
    let stop = MenuItem::new(t!("tray.stop"), false, None);
    let quit = MenuItem::new(t!("tray.quit"), true, None);
    menu.append_items(&[
        &status,
        &open,
        &sharing,
        &start,
        &pair,
        &stop,
        &PredefinedMenuItem::separator(),
        &quit,
    ])?;
    let icon = image::load_from_memory(include_bytes!(
        "../../tray/Sources/RemoventTray/Resources/AppIcon.png"
    ))?
    .resize(32, 32, image::imageops::FilterType::Lanczos3)
    .to_rgba8();
    let _tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Removent")
        .with_icon(tray_icon::Icon::from_rgba(icon.into_raw(), 32, 32)?)
        .build()?;
    let rt = tokio::runtime::Runtime::new()?;
    let (requests, rx) = tokio::sync::mpsc::unbounded_channel();
    let (events, updates) = mpsc::channel();
    rt.spawn(link(paths.clone(), rx, events));
    let smoke = std::env::args().any(|a| a == "--smoke");
    let started = Instant::now();
    let mut prompts = crate::prompts::Prompts::default();
    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        if smoke && started.elapsed() > Duration::from_secs(2) {
            *flow = ControlFlow::Exit;
            return;
        }
        if !matches!(event, Event::UserEvent(())) {
            return;
        }
        // Native check items toggle before queuing MenuEvent. Capture that
        // click intent before a queued daemon status can repaint the checkmark.
        let actions: Vec<_> = MenuEvent::receiver()
            .try_iter()
            .map(|event| {
                let sharing_on = (event.id == sharing.id()).then(|| sharing.is_checked());
                (event, sharing_on)
            })
            .collect();
        while let Ok(value) = updates.try_recv() {
            if value.is_null() {
                prompts.disconnected();
                status.set_text(t!("tray.offline"));
                sharing.set_enabled(false);
                pair.set_enabled(false);
                stop.set_enabled(false);
                start.set_enabled(true);
                continue;
            }
            if let Ok(IpcResponse::Status(report)) = serde_json::from_value(value.clone()) {
                status.set_text(if let Some(error) = report.host_error {
                    error
                } else {
                    t!(
                        "tray.status",
                        state = if report.host_ready {
                            t!("tray.sharing")
                        } else {
                            t!("tray.stopped")
                        },
                        count = report.sessions.len()
                    )
                    .into_owned()
                });
                sharing.set_enabled(true);
                sharing.set_checked(report.running);
                pair.set_enabled(true);
                stop.set_enabled(true);
                start.set_enabled(false);
            } else if let Ok(IpcResponse::PairingCode { code, .. }) =
                serde_json::from_value(value.clone())
            {
                prompts.pairing(code, &t!("dialog.pairing"));
            } else if let Ok(IpcResponse::Error { message }) = serde_json::from_value(value.clone())
            {
                prompts.alert(message);
            } else if let Ok(IpcEvent::PairingPin { pin }) = serde_json::from_value(value.clone()) {
                prompts.pairing(pin, &t!("dialog.pairing"));
            } else if let Ok(IpcEvent::PairingCleared) = serde_json::from_value(value.clone()) {
                prompts.reset_pairing();
            } else if let Ok(IpcEvent::StateChanged { .. }) = serde_json::from_value(value.clone())
            {
                let _ = requests.send(IpcRequest::Status);
            } else if let Ok(IpcEvent::AdmissionResolved { request_id, .. }) =
                serde_json::from_value(value.clone())
            {
                prompts.resolved(request_id);
            } else if let Ok(IpcEvent::AdmissionRequest {
                request_id,
                peer_name,
                peer_fp16,
            }) = serde_json::from_value(value)
            {
                prompts.admission(
                    request_id,
                    &peer_name,
                    &peer_fp16,
                    &t!("dialog.admission"),
                    removent_core::desktop_instance::DesktopInstance::is_running(&paths),
                );
            }
        }
        for (event, sharing_on) in actions {
            let result = if event.id == open.id() {
                launch("removent", &paths)
            } else if event.id == start.id() {
                removent_core::service_intent::set_stopped(&paths, false)
                    .and_then(|_| launch("removentd", &paths))
            } else if let Some(on) = sharing_on {
                let _ = requests.send(IpcRequest::SetEnabled { on });
                sharing.set_checked(on);
                Ok(())
            } else if event.id == pair.id() {
                prompts.reset_pairing();
                let _ = requests.send(IpcRequest::PairingGenerate);
                Ok(())
            } else if event.id == stop.id() {
                let _ = requests.send(IpcRequest::Shutdown);
                Ok(())
            } else if event.id == quit.id() {
                *flow = ControlFlow::Exit;
                Ok(())
            } else {
                Ok(())
            };
            if let Err(e) = result {
                prompts.alert(e.to_string());
            }
        }
        prompts.poll(&requests);
        // Keep runtime alive for the IPC worker throughout the event loop.
        let _ = &rt;
    });
}
async fn link(
    paths: DataPaths,
    mut commands: tokio::sync::mpsc::UnboundedReceiver<IpcRequest>,
    events: mpsc::Sender<serde_json::Value>,
) {
    let mut connected_once = false;
    let mut restart_after = Instant::now();
    loop {
        if let Ok((mut r, mut w)) = removent_core::ipc::connect(&paths).await {
            connected_once = true;
            let mut timer = tokio::time::interval(Duration::from_secs(2));
            let mut reader = removent_core::ipc::MessageReader::default();
            loop {
                tokio::select! {
                    _=timer.tick()=>if write_msg(&mut w,&IpcRequest::Status).await.is_err(){break;},
                    command=commands.recv()=>match command { Some(command)=>if write_msg(&mut w,&command).await.is_err(){break;},None=>return },
                    event=reader.read::<_,serde_json::Value>(&mut r)=>match event {Ok(Some(value))=>{let _=events.send(value);},_=>break},
                }
            }
        }
        let _ = events.send(serde_json::Value::Null);
        while commands.try_recv().is_ok() {}
        // Recover an unexpectedly lost service, while preserving an explicit
        // stop. The child checks the intent again before acquiring its singleton.
        if connected_once
            && Instant::now() >= restart_after
            && removent_core::service_intent::is_stopped(&paths).is_ok_and(|stopped| !stopped)
        {
            let _ = launch("removentd", &paths);
            restart_after = Instant::now() + Duration::from_secs(5);
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
