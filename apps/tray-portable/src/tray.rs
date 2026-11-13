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
    let mut child = std::process::Command::new(sibling(name)?)
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
fn alert(title: &str, text: &str) {
    rfd::MessageDialog::new()
        .set_title(title)
        .set_description(text)
        .show();
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
    let event_loop = EventLoopBuilder::new().build();
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
    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(150));
        if smoke && started.elapsed() > Duration::from_secs(2) {
            *flow = ControlFlow::Exit;
            return;
        }
        if !matches!(event, Event::MainEventsCleared) {
            return;
        }
        while let Ok(value) = updates.try_recv() {
            if value.is_null() {
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
                alert(label("Pairing code", "配对码"), &code);
            } else if let Ok(IpcResponse::Error { message }) = serde_json::from_value(value.clone())
            {
                alert("Removent", &message);
            } else if let Ok(IpcEvent::PairingPin { pin }) = serde_json::from_value(value.clone()) {
                alert(label("Pairing code", "配对码"), &pin);
            } else if let Ok(IpcEvent::AdmissionRequest {
                request_id,
                peer_name,
                peer_fp16,
            }) = serde_json::from_value(value)
            {
                let allow = rfd::MessageDialog::new()
                    .set_title(label("Remote access request", "远程访问请求"))
                    .set_description(format!("{peer_name}\n{peer_fp16}"))
                    .set_buttons(rfd::MessageButtons::YesNo)
                    .show()
                    == rfd::MessageDialogResult::Yes;
                let _ = requests.send(IpcRequest::AdmissionReply { request_id, allow });
            }
        }
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            let result = if event.id == open.id() {
                launch("removent", &paths)
            } else if event.id == start.id() {
                removent_core::service_intent::set_stopped(&paths, false)
                    .map_err(anyhow::Error::from)
                    .and_then(|_| launch("removentd", &paths))
            } else if event.id == sharing.id() {
                let _ = requests.send(IpcRequest::SetEnabled {
                    on: sharing.is_checked(),
                });
                Ok(())
            } else if event.id == pair.id() {
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
                alert("Removent", &e.to_string());
            }
        }
        // Keep runtime alive for the IPC worker throughout the event loop.
        let _ = &rt;
    });
}
async fn link(
    paths: DataPaths,
    mut commands: tokio::sync::mpsc::UnboundedReceiver<IpcRequest>,
    events: mpsc::Sender<serde_json::Value>,
) {
    loop {
        if let Ok((mut r, mut w)) = removent_core::ipc::connect(&paths).await {
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
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
