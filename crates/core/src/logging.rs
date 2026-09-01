//! Logging initialization (FR-63): stdout + rolling file, RUST_LOG overrides levels.

use crate::paths::DataPaths;
use std::fs::File as StdFile;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// Initialize global tracing. Safe to call repeatedly (only the first call takes effect).
pub fn init_logging(paths: &DataPaths) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,quinn=warn,rustls=warn"));

    let stdout_layer = tracing_subscriber::fmt::layer()
        .compact()
        .with_writer(std::io::stdout)
        .with_filter(filter.clone())
        .boxed();

    let _ = tracing_subscriber::registry()
        .with(stdout_layer)
        .with(file_layer(paths))
        .try_init();
}

fn file_layer<S>(paths: &DataPaths) -> Option<impl tracing_subscriber::Layer<S>>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    let dir = paths.logs_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("removent.log");
    // Simple size-based rotation: rotate to .1 when >16MB (keep 2 copies).
    rotate_if_needed(&path, 16 * 1024 * 1024);
    let file = StdFile::options()
        .append(true)
        .create(true)
        .open(path)
        .ok()?;
    Some(
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(file)),
    )
}

fn rotate_if_needed(path: &std::path::Path, max_bytes: u64) {
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    if meta.len() < max_bytes {
        return;
    }
    let one = path.with_extension("log.1");
    let two = path.with_extension("log.2");
    let _ = std::fs::rename(&one, &two);
    let _ = std::fs::rename(path, &one);
}

/// panic hook: write panics into userdata/logs/panics/ (no private content).
pub fn install_panic_hook(paths: &DataPaths) {
    let panics_dir = paths.panics_dir();
    std::panic::set_hook(Box::new(move |info| {
        let ts = chrono_like_ts();
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned());
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();
        let thread = std::thread::current()
            .name()
            .unwrap_or("<unnamed>")
            .to_string();
        let text = format!("ts={ts}\nthread={thread}\nlocation={location}\nmessage={msg:?}\n\n");
        let dir = panics_dir.clone();
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join(format!("panic-{ts}.txt")), text);
    }));
}

fn chrono_like_ts() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}-{:03}", d.as_secs(), d.subsec_millis())
}
