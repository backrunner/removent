//! Local diagnostics: filtered console + bounded, process-safe rolling files.

use crate::paths::DataPaths;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::{fs::OpenOptionsExt, fs::PermissionsExt, io::AsRawFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tracing::field::{Field, Visit};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::field::{RecordFields, VisitOutput};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::fmt::format::{DefaultVisitor, FormatFields, Writer};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const MAX_LOG_BYTES: u64 = 8 * 1024 * 1024;
const LOG_BACKUPS: usize = 3;
const MAX_EVENT_BYTES: usize = 64 * 1024;
const MAX_PANIC_REPORTS: usize = 10;
const TRUNCATED: &[u8] = b" [truncated]\n";

/// Initialize global tracing. Only the first subscriber takes effect.
/// App and daemon have separate files; multiple instances serialize each append
/// and rotation through flock, then reopen the active file (never a stale inode).
pub fn init_logging(paths: &DataPaths) {
    let component = component_name();
    let path = paths.logs_dir().join(format!("{component}.log"));
    let file = RollingLog::new(path.clone(), MAX_LOG_BYTES, LOG_BACKUPS);
    let file = match file {
        Ok(file) => Some(file),
        Err(error) => {
            eprintln!(
                "Removent: cannot write local logs at {}: {error}",
                path.display()
            );
            None
        }
    };
    let file_enabled = file.is_some();
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,quinn=warn,rustls=warn"));
    let console = tracing_subscriber::fmt::layer()
        .fmt_fields(SafeFields)
        .with_ansi(false)
        .with_writer(std::io::stderr);
    let file = file.map(|writer| {
        tracing_subscriber::fmt::layer()
            .fmt_fields(SafeFields)
            .with_ansi(false)
            .with_thread_ids(true)
            .with_writer(writer)
    });
    if tracing_subscriber::registry()
        .with(filter)
        .with(console)
        .with(file)
        .try_init()
        .is_ok()
    {
        tracing::info!(
            component,
            version = env!("CARGO_PKG_VERSION"),
            pid = std::process::id(),
            os = std::env::consts::OS,
            arch = std::env::consts::ARCH,
            file_enabled,
            log_path = %path.display(),
            "local diagnostics initialized"
        );
    }
}

fn component_name() -> &'static str {
    match std::env::current_exe()
        .ok()
        .as_deref()
        .and_then(Path::file_stem)
        .and_then(|s| s.to_str())
    {
        Some("removentd") => "removentd",
        Some("removent-cli") => "removent-cli",
        _ => "removent",
    }
}

/// Defense in depth for structured fields. Free-form messages must never contain
/// credentials, PINs, clipboard contents, key events, or complete request/config dumps.
#[derive(Clone, Copy)]
struct SafeFields;

impl<'writer> FormatFields<'writer> for SafeFields {
    fn format_fields<R: RecordFields>(
        &self,
        writer: Writer<'writer>,
        fields: R,
    ) -> std::fmt::Result {
        struct Visitor<'a>(DefaultVisitor<'a>);
        impl Visit for Visitor<'_> {
            fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                let sensitive = field
                    .name()
                    .to_ascii_lowercase()
                    .split(['_', '.'])
                    .any(|part| {
                        matches!(
                            part,
                            "password"
                                | "passwd"
                                | "pin"
                                | "token"
                                | "secret"
                                | "authorization"
                                | "cookie"
                                | "clipboard"
                                | "credentials"
                        )
                    });
                if sensitive {
                    self.0.record_str(field, "[REDACTED]");
                } else {
                    self.0.record_debug(field, value);
                }
            }
        }
        let mut visitor = Visitor(DefaultVisitor::new(writer, true));
        fields.record(&mut visitor);
        visitor.0.finish()
    }
}

fn private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

fn private_file(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

/// A fresh descriptor per acquisition also serializes independent threads in one process.
fn lock_file(path: &Path) -> io::Result<File> {
    let file = private_file(path)?;
    loop {
        // SAFETY: file owns a valid descriptor and remains alive for the lock's lifetime.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
            return Ok(file);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

struct RollingLog {
    path: PathBuf,
    max_bytes: u64,
    backups: usize,
    write_failed: AtomicBool,
}

impl RollingLog {
    fn new(path: PathBuf, max_bytes: u64, backups: usize) -> io::Result<Self> {
        private_dir(
            path.parent()
                .ok_or_else(|| io::Error::other("missing log directory"))?,
        )?;
        private_file(&path)?;
        Ok(Self {
            path,
            max_bytes,
            backups,
            write_failed: AtomicBool::new(false),
        })
    }

    fn backup(&self, index: usize) -> PathBuf {
        self.path.with_extension(format!("log.{index}"))
    }

    fn append(&self, bytes: &[u8]) -> io::Result<()> {
        let _lock = lock_file(&self.path.with_extension("log.lock"))?;
        let file = private_file(&self.path)?;
        let len = file.metadata()?.len();
        drop(file);
        if len > 0 && len.saturating_add(bytes.len() as u64) > self.max_bytes {
            for index in (1..=self.backups).rev() {
                let from = if index == 1 {
                    self.path.clone()
                } else {
                    self.backup(index - 1)
                };
                match fs::rename(from, self.backup(index)) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
            }
        }
        private_file(&self.path)?.write_all(bytes)
    }
}

/// Format a whole event before taking the file lock. This prevents interleaved
/// records and bounds oversized events. Each event reaches the OS before return.
struct LogEvent<'a> {
    log: &'a RollingLog,
    bytes: Vec<u8>,
    truncated: bool,
}

impl<'a> MakeWriter<'a> for RollingLog {
    type Writer = LogEvent<'a>;
    fn make_writer(&'a self) -> Self::Writer {
        LogEvent {
            log: self,
            bytes: Vec::new(),
            truncated: false,
        }
    }
}

impl Write for LogEvent<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let available = MAX_EVENT_BYTES - TRUNCATED.len() - self.bytes.len();
        self.bytes
            .extend_from_slice(&buf[..buf.len().min(available)]);
        self.truncated |= buf.len() > available;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for LogEvent<'_> {
    fn drop(&mut self) {
        if self.truncated {
            // Truncation can cut a UTF-8 code point; keep the file valid text.
            while std::str::from_utf8(&self.bytes).is_err() && !self.bytes.is_empty() {
                self.bytes.pop();
            }
            self.bytes.extend_from_slice(TRUNCATED);
        }
        if self.bytes.is_empty() {
            return;
        }
        if let Err(error) = self.log.append(&self.bytes) {
            // Never log through tracing here: doing so recursively enters this writer.
            if !self.log.write_failed.swap(true, Ordering::Relaxed) {
                eprintln!("Removent: local log write failed: {error}");
            }
        } else {
            self.log.write_failed.store(false, Ordering::Relaxed);
        }
    }
}

/// Save bounded crash reports with location and stack. Panic payloads can embed
/// arbitrary user data, so they are deliberately omitted. The independent writer
/// remains usable if the tracing subscriber itself panics.
pub fn install_panic_hook(paths: &DataPaths) {
    let dir = paths.panics_dir();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();
        let text = format!(
            "version={}\ncomponent={}\npid={}\nos={}\narch={}\nthread={:?}\nlocation={}\nmessage=[omitted for privacy]\n\n{}\n",
            env!("CARGO_PKG_VERSION"),
            component_name(),
            std::process::id(),
            std::env::consts::OS,
            std::env::consts::ARCH,
            std::thread::current().name(),
            location,
            std::backtrace::Backtrace::force_capture(),
        );
        match write_panic_report(&dir, &text) {
            Ok(path) => eprintln!(
                "Removent panicked at {location}; diagnostic report: {}",
                path.display()
            ),
            Err(error) => {
                eprintln!("Removent panicked at {location}; cannot save crash report: {error}")
            }
        }
    }));
}

fn write_panic_report(dir: &Path, text: &str) -> io::Result<PathBuf> {
    private_dir(dir)?;
    let _lock = lock_file(&dir.join(".lock"))?;
    let mut reports = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("panic-") && n.ends_with(".txt"))
        })
        .collect::<Vec<_>>();
    reports.sort();
    for path in reports
        .iter()
        .take(reports.len().saturating_sub(MAX_PANIC_REPORTS - 1))
    {
        fs::remove_file(path)?;
    }
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = dir.join(format!(
        "panic-{ts}-{}-{:06}.txt",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut end = text.len().min(MAX_EVENT_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    private_file(&path)?.write_all(&text.as_bytes()[..end])?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn rotation_happens_during_writes_and_retention_is_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let log = RollingLog::new(dir.path().join("removent.log"), 10, 2).unwrap();
        for record in ["first\n", "second\n", "third\n", "fourth\n"] {
            log.make_writer().write_all(record.as_bytes()).unwrap();
        }
        assert_eq!(fs::read_to_string(&log.path).unwrap(), "fourth\n");
        assert_eq!(fs::read_to_string(log.backup(1)).unwrap(), "third\n");
        assert_eq!(fs::read_to_string(log.backup(2)).unwrap(), "second\n");
        assert!(!log.backup(3).exists());
    }

    #[test]
    fn independent_writers_reopen_after_rotation_without_losing_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("removent.log");
        let first = Arc::new(RollingLog::new(path.clone(), 1024, 30).unwrap());
        let second = Arc::new(RollingLog::new(path, 1024, 30).unwrap());
        std::thread::scope(|scope| {
            for (id, log) in [(0, &first), (1, &second)] {
                scope.spawn(move || {
                    for n in 0..500 {
                        writeln!(log.make_writer(), "{id}-{n}").unwrap();
                    }
                });
            }
        });
        let mut records = String::new();
        for path in std::iter::once(first.path.clone()).chain((1..=30).map(|i| first.backup(i))) {
            if path.exists() {
                records.push_str(&fs::read_to_string(path).unwrap());
            }
        }
        let records = records.lines().collect::<std::collections::HashSet<_>>();
        assert_eq!(records.len(), 1000);
        for id in 0..2 {
            for n in 0..500 {
                assert!(records.contains(format!("{id}-{n}").as_str()));
            }
        }
    }

    #[test]
    fn file_filter_and_secret_fields_apply_to_events_and_spans() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("removent.log");
        let subscriber = tracing_subscriber::registry()
            .with(EnvFilter::new("info"))
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .fmt_fields(SafeFields)
                    .with_writer(RollingLog::new(path.clone(), MAX_LOG_BYTES, 3).unwrap()),
            );
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("connection", auth_token = "span-secret");
            let _entered = span.enter();
            tracing::debug!("hidden debug detail");
            tracing::info!(
                password = "password-secret",
                pin = 812941,
                clipboard_text = "clipboard-secret",
                port = 5900,
                "connecting"
            );
            tracing::warn!("connection timed out");
            let paths = DataPaths {
                root: dir.path().to_path_buf(),
            };
            fs::write(
                paths.settings_file(),
                "vnc_password = \"settings-secret-without-closing-quote",
            )
            .unwrap();
            crate::Settings::load(&paths).unwrap();
        });
        let text = fs::read_to_string(path).unwrap();
        for secret in [
            "span-secret",
            "password-secret",
            "812941",
            "clipboard-secret",
            "hidden debug detail",
            "settings-secret-without-closing-quote",
        ] {
            assert!(!text.contains(secret), "leaked {secret}");
        }
        assert!(text.contains("[REDACTED]"));
        assert!(text.contains("5900"));
        assert!(text.contains("connection timed out"));
        assert!(text.contains("byte_offset="));
        assert!(!text.contains('\u{1b}'));
    }

    #[test]
    fn oversized_events_are_bounded_valid_text() {
        let dir = tempfile::tempdir().unwrap();
        let log = RollingLog::new(dir.path().join("removent.log"), MAX_LOG_BYTES, 3).unwrap();
        log.make_writer()
            .write_all("中文".repeat(MAX_EVENT_BYTES).as_bytes())
            .unwrap();
        let text = fs::read_to_string(&log.path).unwrap();
        assert!(text.len() <= MAX_EVENT_BYTES);
        assert!(text.ends_with(" [truncated]\n"));
    }

    #[test]
    fn crash_reports_have_private_permissions_and_bounded_retention() {
        let dir = tempfile::tempdir().unwrap();
        let panics = dir.path().join("panics");
        fs::create_dir(&panics).unwrap();
        fs::write(panics.join("unrelated.txt"), "keep").unwrap();
        for _ in 0..15 {
            write_panic_report(&panics, "location=test\nstack\n").unwrap();
        }
        let reports = fs::read_dir(&panics)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("panic-"))
            .collect::<Vec<_>>();
        assert_eq!(reports.len(), MAX_PANIC_REPORTS);
        assert_eq!(
            fs::metadata(&panics).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for report in reports {
            assert_eq!(
                report.metadata().unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert!(panics.join("unrelated.txt").exists());
    }

    #[test]
    fn panic_hook_child() {
        let Some(root) = std::env::var_os("REMOVENT_PANIC_TEST_DIR") else {
            return;
        };
        let paths = DataPaths { root: root.into() };
        install_panic_hook(&paths);
        panic!("panic-payload-secret-must-not-be-saved");
    }

    #[test]
    fn panic_hook_saves_stack_without_payload_in_an_isolated_process() {
        let dir = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "logging::tests::panic_hook_child", "--nocapture"])
            .env("REMOVENT_PANIC_TEST_DIR", dir.path())
            .output()
            .unwrap();
        assert!(!output.status.success());
        let report = fs::read_dir(dir.path().join("logs/panics"))
            .unwrap()
            .flatten()
            .find(|entry| entry.file_name().to_string_lossy().starts_with("panic-"))
            .unwrap();
        let text = fs::read_to_string(report.path()).unwrap();
        assert!(text.contains("location="));
        assert!(text.contains("logging::tests::panic_hook_child"));
        assert!(!text.contains("panic-payload-secret"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("panic-payload-secret"));
    }

    #[test]
    fn unavailable_log_directory_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file");
        fs::write(&file, "occupied").unwrap();
        assert!(RollingLog::new(file.join("removent.log"), MAX_LOG_BYTES, 3).is_err());
    }
}
