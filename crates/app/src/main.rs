//! Removent app entry: GPUI interface + tokio engine bridge.

mod assets;
mod audio;
mod discovery;
mod engine;
mod permissions;
mod theme;
mod ui;
mod updater;

use anyhow::Result;
use gpui::{AppContext, KeyBinding};
use gpui_component::TitleBar;
use removent_core::{Theme as ThemePref, logging, paths::DataPaths};
use ui::connection::{ConnectionTab, ConnectionTabPrev};
use ui::home::{HomeConnect, HomeEscape, HomeSearch, HomeSettings};
use ui::viewer::{ViewerEscape, ViewerToggleFullscreen, ViewerToggleInfo};

rust_i18n::i18n!("locales");

fn main() -> Result<()> {
    let paths = DataPaths::resolve();
    logging::init_logging(&paths);
    logging::install_panic_hook(&paths);
    let result = run(paths);
    if let Err(error) = &result {
        tracing::error!(error = %format!("{error:#}"), "application exited with an error");
    }
    result
}

fn run(paths: DataPaths) -> Result<()> {
    let started = std::time::Instant::now();
    let settings = removent_core::Settings::load(&paths).unwrap_or_else(|error| {
        tracing::warn!(%error, "could not load settings; using defaults");
        removent_core::Settings::default()
    });
    rust_i18n::set_locale(removent_core::resolve_locale(settings.language));

    // Engine runtime (separate thread; the UI interacts via channels).
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()?;

    let engine = engine::Engine::new(rt, paths.clone(), settings);
    engine.start_background_daemon();
    // Tray process lookup/launch must not delay the first window.
    let tray_paths = paths.clone();
    engine.rt.spawn_blocking(move || autostart_tray(tray_paths));

    engine.start_discovery();

    let engine_for_window = engine.clone();
    let theme_pref = engine.settings().theme;
    gpui::Application::new()
        .with_assets(assets::Assets)
        .run(move |cx: &mut gpui::App| {
            gpui_component::init(cx);

            // Theme: pick light/dark per settings, then overlay the "Quiet Control" tokens.
            {
                use gpui_component::theme::{Theme, ThemeMode};
                let dark = match theme_pref {
                    ThemePref::Dark => true,
                    ThemePref::Light => false,
                    ThemePref::System => matches!(
                        cx.window_appearance(),
                        gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark
                    ),
                };
                Theme::change(
                    if dark {
                        ThemeMode::Dark
                    } else {
                        ThemeMode::Light
                    },
                    None,
                    cx,
                );
                theme::apply(dark, cx);
            }

            cx.bind_keys([
                KeyBinding::new("tab", ConnectionTab, Some("ConnectionDialog")),
                KeyBinding::new("shift-tab", ConnectionTabPrev, Some("ConnectionDialog")),
                KeyBinding::new("ctrl-cmd-escape", ViewerEscape, Some("Viewer")),
                KeyBinding::new("ctrl-cmd-f", ViewerToggleFullscreen, Some("Viewer")),
                KeyBinding::new("ctrl-cmd-i", ViewerToggleInfo, Some("Viewer")),
                KeyBinding::new("escape", HomeEscape, Some("Home")),
                KeyBinding::new("cmd-,", HomeSettings, Some("Home")),
                KeyBinding::new("cmd-l", HomeConnect, Some("Home")),
                KeyBinding::new("cmd-f", HomeSearch, Some("Home")),
            ]);

            cx.open_window(
                gpui::WindowOptions {
                    window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::centered(
                        None,
                        gpui::size(gpui::px(1040.), gpui::px(700.)),
                        cx,
                    ))),
                    titlebar: Some(TitleBar::title_bar_options()),
                    window_min_size: Some(gpui::size(gpui::px(860.), gpui::px(600.))),
                    window_background: gpui::WindowBackgroundAppearance::Opaque,
                    ..Default::default()
                },
                move |window, cx| {
                    let _ = paths.ensure_layout();
                    // Two-machine debugging aid: REMOVENT_AUTOSTART_HOST=1 auto-enables
                    // the host service.
                    if std::env::var("REMOVENT_AUTOSTART_HOST").as_deref() == Ok("1") {
                        // Enable the host service via the daemon (spawning it when offline).
                        engine_for_window.set_host_enabled(true);
                        tracing::info!("host service autostart requested (env)");
                    }
                    // Note: the engine itself must be moved (events_rx can only be taken
                    // once; a clone holds an empty one).
                    let view = cx.new(|cx| ui::HomeView::new(engine_for_window, window, cx));
                    window.on_next_frame(move |_, _| {
                        // Preserve rollback until the new app has rendered a frame.
                        updater::cleanup_stale_backup();
                        tracing::info!(
                            elapsed_ms = started.elapsed().as_millis(),
                            "home first frame"
                        );
                    });
                    // gpui-component's Input/Dialog etc. require the window root to be Root.
                    cx.new(|cx| gpui_component::Root::new(gpui::AnyView::from(view), window, cx))
                },
            )
            .expect("open home window");
            cx.activate(true);
        });
    Ok(())
}

/// Launch the tray (RemoventTray) at startup.
///
/// The tray is an independent process: the main app exiting does not take it down; only the
/// user can quit it explicitly from the tray menu. Not launched again when already running.
/// `REMOVENT_NO_TRAY=1` disables this (escape hatch for automated tests).
fn autostart_tray(paths: DataPaths) {
    if std::env::var("REMOVENT_NO_TRAY").as_deref() == Ok("1") {
        return;
    }
    let data_root = if paths.root.is_absolute() {
        paths.root
    } else {
        std::env::current_dir().unwrap_or_default().join(paths.root)
    };
    // The tray owns a per-data-directory flock. Process-name lookup would let
    // an unrelated development tray suppress the installed tray and is racy.
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(exe_dir) = exe.parent() else { return };

    // .app candidates: Helpers inside the main app bundle (release form), then the
    // bundle's sibling directory (running from dist/ directly).
    let mut app_candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Some(contents) = exe_dir.parent() {
        app_candidates.push(contents.join("Helpers/RemoventTray.app"));
        if let Some(bundle) = contents.parent()
            && let Some(dist) = bundle.parent()
        {
            app_candidates.push(dist.join("RemoventTray.app"));
        }
    }
    for app in app_candidates {
        if !app.exists() {
            continue;
        }
        match std::process::Command::new("open")
            .arg("-g")
            .arg("-n")
            .arg("--env")
            .arg(format!("REMOVENT_DATA_DIR={}", data_root.display()))
            .arg(&app)
            .status()
        {
            Ok(status) if status.success() => {
                tracing::info!(path=%app.display(), "RemoventTray launched");
                return;
            }
            Ok(status) => tracing::warn!(%status, path=%app.display(), "open RemoventTray failed"),
            Err(e) => tracing::warn!(err=%e, path=%app.display(), "open RemoventTray failed"),
        }
    }

    // Use the source root, not target/../..: CARGO_TARGET_DIR may be elsewhere.
    // Match the running profile so an old release tray cannot shadow debug UI.
    if let Some(ws) = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
    {
        let profiles = if cfg!(debug_assertions) {
            ["debug", "release"]
        } else {
            ["release", "debug"]
        };
        for profile in profiles {
            let bin = ws.join(format!("tray/.build/{profile}/RemoventTray"));
            if !bin.is_file() {
                continue;
            }
            match std::process::Command::new(&bin)
                .env("REMOVENT_DATA_DIR", &data_root)
                .env("REMOVENT_SERVICE_CLI", exe_dir.join("removent-cli"))
                .env("REMOVENT_DEV_APP", &exe)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                Ok(mut child) => {
                    std::thread::spawn(move || {
                        if let Ok(status) = child.wait()
                            && !status.success()
                        {
                            tracing::warn!(%status, "RemoventTray exited unexpectedly");
                        }
                    });
                    tracing::info!(path=%bin.display(), "RemoventTray spawned (dev)");
                    return;
                }
                Err(e) => tracing::warn!(err=%e, path=%bin.display(), "spawn RemoventTray failed"),
            }
        }
    }
    tracing::warn!("RemoventTray not found; tray not started");
}
