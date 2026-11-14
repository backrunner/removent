use removent_core::{
    DataDirLock, DataPaths,
    ipc::{IpcEvent, IpcRequest, IpcResponse, write_msg},
};
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
    let zh = removent_core::resolve_locale(settings.language).starts_with("zh");
    let label = move |en, cn| if zh { cn } else { en };
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
    let status = MenuItem::new(
        label("Connecting to background service", "正在连接后台服务"),
        false,
        None,
    );
    let open = MenuItem::new(label("Open Removent", "打开 Removent"), true, None);
    let sharing = CheckMenuItem::new(label("Share this desktop", "共享本机"), false, false, None);
    let start = MenuItem::new(
        label("Start background service", "启动后台服务"),
        true,
        None,
    );
    let pair = MenuItem::new(label("Show pairing code", "显示配对码"), false, None);
    let stop = MenuItem::new(
        label("Stop background service", "停止后台服务"),
        false,
        None,
    );
    let quit = MenuItem::new(label("Quit tray", "退出托盘"), true, None);
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
        while let Ok(value) = updates.try_recv() {
            if value.is_null() {
                prompts.disconnected();
                status.set_text(label("Background service offline", "后台服务未连接"));
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
                    format!(
                        "{} ({})",
                        if report.host_ready {
                            label("Sharing", "正在共享")
                        } else {
                            label("Sharing stopped", "共享已停止")
                        },
                        report.sessions.len()
                    )
                });
                sharing.set_enabled(true);
                sharing.set_checked(report.running);
                pair.set_enabled(true);
                stop.set_enabled(true);
                start.set_enabled(false);
            } else if let Ok(IpcResponse::PairingCode { code, .. }) =
                serde_json::from_value(value.clone())
            {
                prompts.pairing(code, label("Pairing code", "配对码"));
            } else if let Ok(IpcResponse::Error { message }) = serde_json::from_value(value.clone())
            {
                prompts.alert(message);
            } else if let Ok(IpcEvent::PairingPin { pin }) = serde_json::from_value(value.clone()) {
                prompts.pairing(pin, label("Pairing code", "配对码"));
            } else if let Ok(IpcEvent::PairingCleared) = serde_json::from_value(value.clone()) {
                prompts.reset_pairing();
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
                    label("Remote access request", "远程访问请求"),
                    removent_core::desktop_instance::DesktopInstance::is_running(&paths),
                );
            }
        }
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            let result = if event.id == open.id() {
                launch("removent", &paths)
            } else if event.id == start.id() {
                removent_core::service_intent::set_stopped(&paths, false)
                    .and_then(|_| launch("removentd", &paths))
            } else if event.id == sharing.id() {
                let _ = requests.send(IpcRequest::SetEnabled {
                    on: sharing.is_checked(),
                });
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
