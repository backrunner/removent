import AppKit
import Darwin

// Kernel-held per-data-directory lock closes the simultaneous desktop/login
// startup race. A development tray cannot suppress the installed user's tray.
let trayData = DaemonClient.dataDirectory()
try FileManager.default.createDirectory(at: trayData, withIntermediateDirectories: true)
let trayLock = open(trayData.appendingPathComponent(".tray.lock").path, O_CREAT | O_RDWR | O_CLOEXEC | O_NOFOLLOW, 0o600)
guard trayLock >= 0 else { exit(1) }
guard flock(trayLock, LOCK_EX | LOCK_NB) == 0 else { close(trayLock); exit(0) }

final class AppDelegate: NSObject, NSApplicationDelegate {
    private var statusBarController: StatusBarController?

    func applicationDidFinishLaunching(_ notification: Notification) {
        statusBarController = StatusBarController()
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let delegate = AppDelegate()
app.delegate = delegate
app.run()
