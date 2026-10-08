import AppKit
import Foundation
import SwiftUI

/// Menu bar controller: owns the NSStatusItem, rebuilds the menu, and handles
/// daemon events and user actions.
final class StatusBarController: NSObject, NSMenuDelegate, NSPopoverDelegate, ObservableObject {
    // Internal state is shared by responsibility-specific extensions in this target.
    let statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
    let menu = NSMenu()
    let client = DaemonClient()
    let popover = NSPopover()
    var popoverScreen: NSScreen?
    var popoverObservers: [NSObjectProtocol] = []
    var confiningPopover = false
    var lastPopoverGeometry: Data?
    @Published var panelWidth: CGFloat = 360
    @Published var panelMaximumHeight: CGFloat = 600

    @Published var connected = false
    @Published var status: StatusResponse?
    @Published var pendingPin: String?
    var pollTimer: Timer?
    @Published var serviceBusy = false
    var serviceQueryBusy = false
    var serviceGeneration = 0
    @Published var loginEnabled = false
    var enableOnConnect = false
    @Published var stoppedByUser = false
    @Published var recoveryError: String?
    var nextRecovery = Date.distantPast
    var recoveryDelay: TimeInterval = 2

    /// Admission requests deferred while the main app is running: if no
    /// admissionResolved broadcast arrives within 8 seconds, the tray shows
    /// the alert itself as a fallback.
    var pendingAdmissionIds = Set<Int>()
    /// Admission requests arriving while an alert is shown are queued and
    /// processed one by one after the current alert closes.
    var pairingAlert: NSAlert?
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
        statusItem.button?.target = self
        statusItem.button?.action = #selector(togglePopover)
        statusItem.button?.sendAction(on: [.leftMouseUp, .rightMouseUp])
        popover.behavior = .transient
        configurePopover()

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
                self.dismissPairingAlert()
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
    @objc func togglePopover() {
        guard let button = statusItem.button else { return }
        if NSApp.currentEvent?.type == .rightMouseUp {
            popover.performClose(nil)
            rebuildMenu()
            statusItem.menu = menu
            button.performClick(nil)
            statusItem.menu = nil
        } else if popover.isShown {
            popover.performClose(nil)
        } else {
            showPanel()
        }
    }

    func showPanel() {
        guard let button = statusItem.button, button.window != nil else { return }
        refreshServiceStatus()
        client.requestStatus()
        updatePopoverLimits()
        if !popover.isShown {
            popover.show(relativeTo: button.bounds, of: button,
                         preferredEdge: button.isFlipped ? .maxY : .minY)
        }
        confinePopover()
        popover.contentViewController?.view.window?.makeKey()
    }

    func introducePermissionsIfNeeded(_ status: StatusResponse) {
        guard !suppressAlerts, status.running,
              !status.screen_recording_granted || !status.accessibility_granted else { return }
        let marker = DaemonClient.dataDirectory().appendingPathComponent("permissions/tray-introduced")
        guard !FileManager.default.fileExists(atPath: marker.path) else { return }
        do {
            try FileManager.default.createDirectory(at: marker.deletingLastPathComponent(), withIntermediateDirectories: true)
            try Data().write(to: marker, options: .atomic)
            showPanel()
        } catch { trayLog("could not record permission introduction: \(error.localizedDescription)") }
    }

}
