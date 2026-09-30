import AppKit
import Foundation

/// Menu bar controller: owns the NSStatusItem, rebuilds the menu, and handles
/// daemon events and user actions.
final class StatusBarController: NSObject, NSMenuDelegate {
    // Internal state is shared by responsibility-specific extensions in this target.
    let statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
    let menu = NSMenu()
    let client = DaemonClient()

    var connected = false
    var status: StatusResponse?
    var pendingPin: String?
    var pollTimer: Timer?
    var serviceBusy = false
    var serviceQueryBusy = false
    var serviceGeneration = 0
    var loginEnabled = false
    var enableOnConnect = false
    var stoppedByUser = false
    var recoveryError: String?
    var nextRecovery = Date.distantPast
    var recoveryDelay: TimeInterval = 2

    /// Admission requests deferred while the main app is running: if no
    /// admissionResolved broadcast arrives within 8 seconds, the tray shows
    /// the alert itself as a fallback.
    var pendingAdmissionIds = Set<Int>()
    /// Admission requests arriving while an alert is shown are queued and
    /// processed one by one after the current alert closes.
    var alertActive = false
    var queuedAdmissionRequests: [(requestId: Int, peerName: String, peerFp16: String)] = []
    /// No rebuilds while the menu is open; mark dirty and rebuild on close.
    var menuOpen = false
    var menuDirty = false

    /// Integration test mode: log instead of showing NSAlert, for automation.
    var suppressAlerts: Bool {
        ProcessInfo.processInfo.environment["REMOVENT_TRAY_NO_ALERTS"] == "1"
    }

    override init() {
        super.init()
        menu.delegate = self
        // Assign the menu once, then rebuild its contents in place to avoid flicker.
        statusItem.menu = menu

        client.onMessage = { [weak self] message in self?.handle(message) }
        client.onConnectionChange = { [weak self] isConnected in
            guard let self else { return }
            let changed = self.connected != isConnected
            self.connected = isConnected
            if isConnected {
                self.recoveryDelay = 2
                self.recoveryError = nil
                self.client.requestStatus()
            }
            if isConnected && self.enableOnConnect {
                self.enableOnConnect = false
                self.client.setEnabled(true)
            }
            if !isConnected {
                self.status = nil
                self.pendingPin = nil
                if changed { trayLog("daemon not running") }
            }
            // Rebuild only when the connection state actually changes; the 1s
            // reconnect attempts while the daemon is offline no longer trigger
            // a full rebuild.
            if changed { self.rebuildMenu() }
            self.updateIcon()
        }
        client.start()
        refreshServiceStatus()

        // 2s status poll as a fallback
        pollTimer = Timer.scheduledTimer(withTimeInterval: 2, repeats: true) { [weak self] _ in
            self?.client.requestStatus()
            self?.watchService()
        }

        rebuildMenu()
        updateIcon()
        trayLog("menu bar UI ready")
    }

    // MARK: - Icon


}
