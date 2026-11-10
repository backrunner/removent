//! NSWorkspace's app list and NSRunningApplication's changing properties depend
//! on the MAIN AppKit run loop. The daemon has no such loop. Enumerate live PIDs,
//! use only immutable app metadata, and query foreground PID without that cache.
use super::*;
use objc2::rc::Retained;
use objc2_app_kit::NSRunningApplication;

#[repr(C)]
#[derive(Default)]
struct ProcessSerialNumber {
    high: u32,
    low: u32,
}
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn GetFrontProcess(serial: *mut ProcessSerialNumber) -> i16;
    fn GetProcessPID(serial: *const ProcessSerialNumber, pid: *mut i32) -> i32;
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    pub pid: i32,
    start: (u64, u64),
}
impl Identity {
    fn read(pid: i32) -> Option<Self> {
        let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
        let size = std::mem::size_of_val(&info) as i32;
        if pid <= 0
            || unsafe {
                libc::proc_pidinfo(
                    pid,
                    libc::PROC_PIDTBSDINFO,
                    0,
                    info.as_mut_ptr().cast(),
                    size,
                )
            } != size
        {
            return None;
        }
        let info = unsafe { info.assume_init() };
        if info.pbi_uid != unsafe { libc::geteuid() } {
            return None;
        }
        Some(Self {
            pid,
            start: (info.pbi_start_tvsec, info.pbi_start_tvusec),
        })
    }
    pub fn alive(self) -> bool {
        Self::read(self.pid) == Some(self)
    }
}
fn running() -> Result<Vec<(Identity, Retained<NSRunningApplication>)>, Failure> {
    // A finite buffer also bounds OS and AppKit metadata work.
    let mut pids = vec![0i32; 65_536];
    let count = unsafe {
        libc::proc_listallpids(
            pids.as_mut_ptr().cast(),
            std::mem::size_of_val(pids.as_slice()) as i32,
        )
    };
    if count <= 0 || count as usize >= pids.len() {
        return Err(Failure::new(
            "applications_unavailable",
            "Cannot enumerate current applications",
        ));
    }
    Ok(pids
        .into_iter()
        .take(count as usize)
        .filter_map(|pid| {
            let identity = Identity::read(pid)?;
            let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)?;
            Some((identity, app))
        })
        .collect())
}
fn frontmost() -> Option<i32> {
    // Public but deprecated Process Manager compatibility API: NSWorkspace's
    // foreground getter also stays stale without a main AppKit run loop. Keep
    // this isolated until the headless host owns such a loop. Never fall back
    // to a cached foreground app when the live query fails.
    let mut serial = ProcessSerialNumber::default();
    let mut pid = 0;
    (unsafe { GetFrontProcess(&mut serial) } == 0
        && unsafe { GetProcessPID(&serial, &mut pid) } == 0
        && pid > 0)
        .then_some(pid)
}
pub fn apps() -> Result<Vec<Value>, Failure> {
    let front = frontmost();
    Ok(running()?
        .into_iter()
        .map(|(identity, app)| {
            json!({
                "target":format!("app:{}",identity.pid), "pid":identity.pid,
                "name":app.localizedName().map(|s|s.to_string()),
                "bundle_id":app.bundleIdentifier().map(|s|s.to_string()),
                "active":front.map(|pid|pid == identity.pid)
            })
        })
        .collect())
}
pub fn resolve(target: &str) -> Result<Identity, Failure> {
    let pid = if target == "frontmost" {
        frontmost()
    } else {
        target.strip_prefix("app:").and_then(|s| s.parse().ok())
    };
    let found = if let Some(pid) = pid {
        Identity::read(pid).filter(|_| {
            NSRunningApplication::runningApplicationWithProcessIdentifier(pid).is_some()
        })
    } else if target == "frontmost" || target.starts_with("app:") {
        None
    } else {
        running()?
            .into_iter()
            .find(|(_, app)| {
                app.bundleIdentifier()
                    .is_some_and(|s| s.to_string() == target)
            })
            .map(|(identity, _)| identity)
    };
    found.ok_or_else(|| {
        Failure::new(
            "app_missing",
            "Target application is not running or cannot be identified",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn process_identity_expires_on_exit_and_rejects_reused_pid() {
        let mut child = std::process::Command::new("/bin/cat")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let identity = Identity::read(child.id() as i32).unwrap();
        assert!(identity.alive());
        let stale = Identity {
            start: (identity.start.0 + 1, identity.start.1),
            ..identity
        };
        assert!(!stale.alive());
        drop(child.stdin.take());
        child.wait().unwrap();
        assert!(!identity.alive());
    }
}
