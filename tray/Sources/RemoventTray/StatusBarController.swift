import AppKit
import Foundation

/// Menu bar controller: owns the NSStatusItem, rebuilds the menu, and handles
/// daemon events and user actions.
final class StatusBarController: NSObject, NSMenuDelegate {

    private let statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
    private let menu = NSMenu()
    private let client = DaemonClient()

    private var connected = false
    private var status: StatusResponse?
    private var pendingPin: String?
    private var pollTimer: Timer?

    /// Admission requests deferred while the main app is running: if no
    /// admissionResolved broadcast arrives within 8 seconds, the tray shows
    /// the alert itself as a fallback.
    private var pendingAdmissionIds = Set<Int>()
    /// Admission requests arriving while an alert is shown are queued and
    /// processed one by one after the current alert closes.
    private var alertActive = false
    private var queuedAdmissionRequests: [(requestId: Int, peerName: String, peerFp16: String)] = []
    /// No rebuilds while the menu is open; mark dirty and rebuild on close.
    private var menuOpen = false
    private var menuDirty = false

    /// Integration test mode: log instead of showing NSAlert, for automation.
    private var suppressAlerts: Bool {
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

        // 2s status poll as a fallback
        pollTimer = Timer.scheduledTimer(withTimeInterval: 2, repeats: true) { [weak self] _ in
            self?.client.requestStatus()
        }

        rebuildMenu()
        updateIcon()
    }

    // MARK: - Icon

    private func updateIcon() {
        let hasSessions = connected && !(status?.sessions.isEmpty ?? true)
        let symbol = hasSessions ? "display.trianglebadge.exclamationmark" : "display"
        if let image = NSImage(systemSymbolName: symbol, accessibilityDescription: "Removent") {
            image.isTemplate = true
            statusItem.button?.image = image
        } else {
            statusItem.button?.title = "Removent"
        }
    }

    // MARK: - Event handling

    private func handle(_ message: DaemonMessage) {
        switch message {
        case .status(let s):
            guard status != s else { return }
            status = s
            pendingPin = s.pending_pin
            trayLog("status: \(s.running ? "running" : "stopped"), port \(s.port), device \(s.device_name), fingerprint \(s.fp_short), sessions \(s.sessions.count)")
            rebuildMenu()
            updateIcon()
        case .ok:
            trayLog("request acknowledged")
            client.requestStatus()
        case .error(let message):
            trayLog("daemon returned error: \(message)")
        case .stateChanged(let running):
            trayLog("service state changed: \(running ? "running" : "stopped")")
            client.requestStatus()
        case .sessionStarted(let session):
            trayLog("session started: \(session.peer_name) (\(session.video_codec))")
            client.requestStatus()
        case .sessionEnded(let id, let reason):
            trayLog("session ended: #\(id), reason: \(reason)")
            client.requestStatus()
        case .admissionRequest(let requestId, let peerName, let peerFp16):
            trayLog("admission request received: \(peerName) (\(peerFp16)), request_id=\(requestId)")
            guard !suppressAlerts else {
                trayLog("(test mode) no alert shown, admission request #\(requestId) left unanswered")
                return
            }
            if mainAppRunning {
                // While the main app is running the main window handles the
                // request first; but the window may be hidden, leaving the
                // request to silently time out, so after 8 seconds without an
                // admissionResolved broadcast the tray shows an alert as a
                // fallback.
                pendingAdmissionIds.insert(requestId)
                trayLog("main app is running; admission request #\(requestId) deferred to main window, tray alert after 8s if unresolved")
                DispatchQueue.main.asyncAfter(deadline: .now() + 8) { [weak self] in
                    guard let self, self.pendingAdmissionIds.remove(requestId) != nil else { return }
                    trayLog("admission request #\(requestId) not handled within 8s, tray showing alert")
                    self.enqueueOrShowAdmissionAlert(requestId: requestId, peerName: peerName, peerFp16: peerFp16)
                }
                return
            }
            enqueueOrShowAdmissionAlert(requestId: requestId, peerName: peerName, peerFp16: peerFp16)
        case .admissionResolved(let requestId, let allow):
            pendingAdmissionIds.remove(requestId)
            // Drop any queued fallback alert for this request: another client
            // (e.g. the main app) already handled it, so presenting it now
            // would be a stale alert.
            queuedAdmissionRequests.removeAll { $0.requestId == requestId }
            trayLog("admission request #\(requestId) resolved: \(allow ? "allowed" : "denied")")
        case .pairingPin(let pin):
            trayLog("pairing PIN received: \(pin)")
            pendingPin = pin
            rebuildMenu()
            showPairingAlert(pin: pin)
        case .pairingDone(let peerName):
            trayLog("pairing completed: \(peerName)")
            pendingPin = nil
            rebuildMenu()
        case .unknown(let type):
            trayLog("ignoring unknown message type: \(type)")
        }
    }

    // MARK: - Alerts

    /// Whether the main app (GPUI window) is running: while running, admission
    /// and pairing alerts are preferably handled by the main app.
    private var mainAppRunning: Bool {
        if let path = ProcessInfo.processInfo.environment["REMOVENT_DEV_APP"] {
            return NSWorkspace.shared.runningApplications.contains { $0.executableURL?.path == path }
        }
        return !NSRunningApplication.runningApplications(withBundleIdentifier: "io.removent.app").isEmpty
    }

    /// Queue admission requests that arrive while an alert is being shown
    /// (the main thread still dispatches events inside runModal) to avoid
    /// nested modals.
    private func enqueueOrShowAdmissionAlert(requestId: Int, peerName: String, peerFp16: String) {
        if alertActive {
            queuedAdmissionRequests.append((requestId, peerName, peerFp16))
            trayLog("alert already active, admission request #\(requestId) queued")
            return
        }
        showAdmissionAlert(requestId: requestId, peerName: peerName, peerFp16: peerFp16)
    }

    private func processQueuedAdmissionAlerts() {
        guard !alertActive, !queuedAdmissionRequests.isEmpty else { return }
        let next = queuedAdmissionRequests.removeFirst()
        showAdmissionAlert(requestId: next.requestId, peerName: next.peerName, peerFp16: next.peerFp16)
    }

    /// Show an admission alert directly (the caller has confirmed the tray
    /// should handle it); sets alertActive for the duration.
    private func showAdmissionAlert(requestId: Int, peerName: String, peerFp16: String) {
        let alert = NSAlert()
        alert.messageText = String(format: String(localized: "alert.connection_request", bundle: .module, comment: "Admission alert title"), peerName)
        alert.informativeText = String(format: String(localized: "alert.connection_request_detail", bundle: .module, comment: "Admission alert body"), peerFp16)
        alert.addButton(withTitle: String(localized: "alert.allow", bundle: .module, comment: "Admission alert allow button"))
        alert.addButton(withTitle: String(localized: "alert.deny", bundle: .module, comment: "Admission alert deny button"))
        alert.alertStyle = .warning
        alertActive = true
        NSApp.activate(ignoringOtherApps: true)
        let allow = alert.runModal() == .alertFirstButtonReturn
        alertActive = false
        client.admissionReply(requestId: requestId, allow: allow)
        processQueuedAdmissionAlerts()
    }

    private func showPairingAlert(pin: String) {
        if suppressAlerts { return }
        if mainAppRunning {
            trayLog("main app is running; pairing PIN \(pin) handled by main window, tray shows no alert")
            return
        }
        let alert = NSAlert()
        alert.messageText = String(localized: "alert.pairing_request", bundle: .module, comment: "Pairing alert title")
        alert.informativeText = String(format: String(localized: "alert.pairing_pin_detail", bundle: .module, comment: "Pairing alert body"), pin)
        alert.addButton(withTitle: String(localized: "alert.ok", bundle: .module, comment: "Alert OK button"))
        alertActive = true
        NSApp.activate(ignoringOtherApps: true)
        alert.runModal()
        alertActive = false
        processQueuedAdmissionAlerts()
    }

    /// Error alert; in test mode only logs.
    private func showErrorAlert(messageText: String, informativeText: String) {
        if suppressAlerts { return }
        let alert = NSAlert()
        alert.messageText = messageText
        alert.informativeText = informativeText
        alert.addButton(withTitle: String(localized: "alert.ok", bundle: .module, comment: "Alert OK button"))
        alert.alertStyle = .warning
        NSApp.activate(ignoringOtherApps: true)
        alert.runModal()
    }

    // MARK: - Menu

    func menuNeedsUpdate(_ menu: NSMenu) {
        // Rebuild when the menu opens so session durations etc. are up to date.
        rebuildMenu()
    }

    func menuWillOpen(_ menu: NSMenu) {
        menuOpen = true
    }

    func menuDidClose(_ menu: NSMenu) {
        menuOpen = false
        if menuDirty {
            menuDirty = false
            rebuildMenu()
        }
    }

    private func rebuildMenu() {
        // removeAllItems while the menu is open causes flicker / lost
        // highlight; mark dirty and rebuild on close instead.
        if menuOpen {
            menuDirty = true
            return
        }
        menu.removeAllItems()

        // 1. Status line
        let stateText: String
        let stateSymbol: String
        if !connected {
            stateText = String(localized: "status.daemon_offline", bundle: .module, comment: "Menu status: daemon not connected")
            stateSymbol = "exclamationmark.circle"
        } else if status?.running == true {
            stateText = String(localized: "status.running", bundle: .module, comment: "Menu status: service running")
            stateSymbol = "checkmark.circle.fill"
        } else {
            stateText = String(localized: "status.stopped", bundle: .module, comment: "Menu status: service stopped")
            stateSymbol = "pause.circle"
        }
        menu.addItem(infoItem(String(format: String(localized: "menu.service_status", bundle: .module, comment: "Menu status line"), stateText), symbol: stateSymbol))
        if let s = status, !s.fp_short.isEmpty {
            menu.addItem(infoItem(String(format: String(localized: "menu.device_fingerprint", bundle: .module, comment: "Menu device fingerprint line"), s.fp_short)))
        }
        // Permission warnings (only relevant while hosting).
        if connected, let s = status, s.running {
            if s.screen_recording_granted == false {
                menu.addItem(infoItem(String(localized: "menu.permission_screen_recording_missing", bundle: .module, comment: "Menu warning: screen recording permission missing"), symbol: "exclamationmark.triangle"))
            }
            if s.accessibility_granted == false {
                menu.addItem(infoItem(String(localized: "menu.permission_accessibility_missing", bundle: .module, comment: "Menu warning: accessibility permission missing"), symbol: "exclamationmark.triangle"))
            }
        }
        menu.addItem(.separator())

        // 1.5 Open main window
        let mainItem = NSMenuItem(title: String(localized: "menu.open_main_window", bundle: .module, comment: "Menu item: open main window"), action: #selector(openMainApp), keyEquivalent: "")
        mainItem.target = self
        mainItem.image = symbolImage("macwindow")
        menu.addItem(mainItem)
        menu.addItem(.separator())

        // 2. Sessions
        let sessions = connected ? (status?.sessions ?? []) : []
        if sessions.isEmpty {
            menu.addItem(infoItem(String(localized: "menu.no_sessions", bundle: .module, comment: "Menu item: no active sessions")))
        } else {
            for session in sessions {
                let title = "\(session.peer_name) · \(durationString(since: session.since_unix)) · \(session.video_codec)"
                menu.addItem(infoItem(title, symbol: "person.fill"))
            }
        }
        menu.addItem(.separator())

        // 3. Pairing PIN
        if let pin = pendingPin, !pin.isEmpty {
            let item = NSMenuItem(title: "", action: nil, keyEquivalent: "")
            let attrs: [NSAttributedString.Key: Any] = [
                .font: NSFont.monospacedDigitSystemFont(ofSize: 20, weight: .semibold)
            ]
            item.attributedTitle = NSAttributedString(string: String(format: String(localized: "menu.pairing_pin", bundle: .module, comment: "Menu pairing PIN line"), pin), attributes: attrs)
            item.isEnabled = false
            menu.addItem(item)
            menu.addItem(.separator())
        }

        // 4. Enable/disable service
        let toggleTitle = (status?.running == true)
            ? String(localized: "menu.disable_service", bundle: .module, comment: "Menu item: disable service")
            : String(localized: "menu.enable_service", bundle: .module, comment: "Menu item: enable service")
        let toggleItem = NSMenuItem(title: toggleTitle, action: #selector(toggleService), keyEquivalent: "")
        toggleItem.target = self
        toggleItem.isEnabled = connected
        toggleItem.image = symbolImage("power")
        menu.addItem(toggleItem)

        // 5. Launch at login
        let loginItem = NSMenuItem(title: String(localized: "menu.launch_at_login", bundle: .module, comment: "Menu item: launch at login"), action: #selector(toggleLaunchAtLogin), keyEquivalent: "")
        loginItem.target = self
        loginItem.state = launchAgentInstalled() ? .on : .off
        loginItem.image = symbolImage("arrow.up.circle")
        menu.addItem(loginItem)

        menu.addItem(.separator())

        // 6. Open data directory
        let openItem = NSMenuItem(title: String(localized: "menu.open_data_directory", bundle: .module, comment: "Menu item: open data directory"), action: #selector(openDataDirectory), keyEquivalent: "")
        openItem.target = self
        openItem.image = symbolImage("folder")
        menu.addItem(openItem)

        // 7. Quit menu bar app
        let quitItem = NSMenuItem(title: String(localized: "menu.quit", bundle: .module, comment: "Menu item: quit tray"), action: #selector(quitTray), keyEquivalent: "q")
        quitItem.target = self
        quitItem.image = symbolImage("xmark.circle")
        menu.addItem(quitItem)
    }

    private func infoItem(_ title: String, symbol: String? = nil) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: nil, keyEquivalent: "")
        item.isEnabled = false
        if let symbol {
            item.image = symbolImage(symbol)
        }
        return item
    }

    private func symbolImage(_ name: String) -> NSImage? {
        let image = NSImage(systemSymbolName: name, accessibilityDescription: nil)
        image?.isTemplate = true
        return image
    }

    private func durationString(since unix: Int) -> String {
        let secs = max(0, Int(Date().timeIntervalSince1970) - unix)
        let h = secs / 3600, m = (secs % 3600) / 60, s = secs % 60
        if h > 0 { return String(format: String(localized: "session.duration.hm", bundle: .module, comment: "Session duration: hours and minutes"), h, m) }
        if m > 0 { return String(format: String(localized: "session.duration.ms", bundle: .module, comment: "Session duration: minutes and seconds"), m, s) }
        return String(format: String(localized: "session.duration.s", bundle: .module, comment: "Session duration: seconds"), s)
    }

    // MARK: - Actions

    @objc private func toggleService() {
        let target = !(status?.running ?? false)
        trayLog("requesting service \(target ? "enable" : "disable")")
        client.setEnabled(target)
    }

    @objc private func openMainApp() {
        if let path = ProcessInfo.processInfo.environment["REMOVENT_DEV_APP"],
           let app = NSWorkspace.shared.runningApplications.first(where: { $0.executableURL?.path == path }) {
            app.activate(options: [.activateAllWindows])
            return
        }
        guard let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "io.removent.app") else {
            trayLog("main app not installed (bundle id io.removent.app not found)")
            showErrorAlert(messageText: String(localized: "alert.main_app_not_found", bundle: .module, comment: "Error alert: main app not installed"),
                           informativeText: String(localized: "alert.main_app_not_found_detail", bundle: .module, comment: "Error alert body: main app not installed"))
            return
        }
        NSWorkspace.shared.openApplication(at: url, configuration: .init()) { [weak self] _, error in
            guard let error else { return }
            trayLog("failed to open main app: \(error.localizedDescription)")
            DispatchQueue.main.async {
                self?.showErrorAlert(messageText: String(localized: "alert.main_app_not_found", bundle: .module, comment: "Error alert: main app not installed"),
                                     informativeText: String(format: String(localized: "alert.open_main_app_failed", bundle: .module, comment: "Error alert body: failed to open main app"), error.localizedDescription))
            }
        }
    }

    @objc private func openDataDirectory() {
        let url = DaemonClient.dataDirectory()
        try? FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        NSWorkspace.shared.open(url)
    }

    @objc private func quitTray() {
        trayLog("quitting menu bar app (daemon unaffected)")
        client.stop()
        NSApp.terminate(nil)
    }

    // MARK: - Launch at login (LaunchAgent)

    private var launchAgentURL: URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/LaunchAgents/com.removent.daemon.plist")
    }

    /// Installed = the plist exists AND launchctl knows the service, so a
    /// leftover plist cannot desync the menu checkbox from reality.
    private func launchAgentInstalled() -> Bool {
        guard FileManager.default.fileExists(atPath: launchAgentURL.path) else { return false }
        return runLaunchctl(["print", "gui/\(getuid())/com.removent.daemon"])
    }

    private func daemonExecutablePath() -> String {
        // The tray is embedded at Removent.app/Contents/Helpers/RemoventTray.app while the
        // daemon ships beside the main executable at Removent.app/Contents/MacOS/removentd —
        // resolve relative to the OUTER bundle, not the tray's own bundle.
        let bundled = Bundle.main.bundleURL
            .deletingLastPathComponent()  // Contents/Helpers/
            .deletingLastPathComponent()  // Contents/
            .appendingPathComponent("MacOS/removentd").path
        if FileManager.default.isExecutableFile(atPath: bundled) { return bundled }
        // Then look it up on PATH
        if let found = which("removentd") { return found }
        for candidate in ["/opt/homebrew/bin/removentd", "/usr/local/bin/removentd"]
        where FileManager.default.isExecutableFile(atPath: candidate) {
            return candidate
        }
        return "/usr/local/bin/removentd"
    }

    private func which(_ name: String) -> String? {
        let task = Process()
        let pipe = Pipe()
        task.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        task.arguments = ["which", name]
        task.standardOutput = pipe
        task.standardError = FileHandle.nullDevice
        guard let _ = try? task.run() else { return nil }
        task.waitUntilExit()
        guard task.terminationStatus == 0 else { return nil }
        let path = String(decoding: pipe.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return path.isEmpty ? nil : path
    }

    private func launchAgentPlist(daemonPath: String, dataDir: String) -> String {
        func escape(_ s: String) -> String {
            s.replacingOccurrences(of: "&", with: "&amp;")
                .replacingOccurrences(of: "<", with: "&lt;")
                .replacingOccurrences(of: ">", with: "&gt;")
        }
        return """
        <?xml version="1.0" encoding="UTF-8"?>
        <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
        <plist version="1.0">
        <dict>
            <key>Label</key>
            <string>com.removent.daemon</string>
            <key>ProgramArguments</key>
            <array>
                <string>\(escape(daemonPath))</string>
            </array>
            <key>EnvironmentVariables</key>
            <dict>
                <key>REMOVENT_DATA_DIR</key>
                <string>\(escape(dataDir))</string>
            </dict>
            <key>RunAtLoad</key>
            <true/>
            <key>KeepAlive</key>
            <true/>
            <key>StandardOutPath</key>
            <string>\(escape(dataDir))/logs/removentd.out.log</string>
            <key>StandardErrorPath</key>
            <string>\(escape(dataDir))/logs/removentd.err.log</string>
        </dict>
        </plist>
        """
    }

    @objc private func toggleLaunchAtLogin() {
        let uid = getuid()
        if launchAgentInstalled() {
            runLaunchctl(["bootout", "gui/\(uid)/com.removent.daemon"])
            try? FileManager.default.removeItem(at: launchAgentURL)
            trayLog("launch at login disabled")
        } else {
            let daemonPath = daemonExecutablePath()
            // daemonExecutablePath falls back to /usr/local/bin, where the file
            // may not exist; abort in that case without writing the plist.
            guard FileManager.default.isExecutableFile(atPath: daemonPath) else {
                trayLog("removentd not found (\(daemonPath)), launch at login not enabled")
                showErrorAlert(messageText: String(localized: "alert.daemon_not_found", bundle: .module, comment: "Error alert: removentd missing"),
                               informativeText: String(localized: "alert.daemon_not_found_detail", bundle: .module, comment: "Error alert body: removentd missing"))
                rebuildMenu()
                return
            }
            do {
                // Pin the data dir explicitly so the launchd-started daemon resolves the
                // same userdata/ location as the app and tray regardless of environment.
                let dataDir = DaemonClient.dataDirectory().path
                try FileManager.default.createDirectory(
                    at: launchAgentURL.deletingLastPathComponent(),
                    withIntermediateDirectories: true)
                // launchd opens the log paths before exec; the logs dir must exist first.
                try FileManager.default.createDirectory(
                    atPath: "\(dataDir)/logs",
                    withIntermediateDirectories: true)
                try launchAgentPlist(daemonPath: daemonPath, dataDir: dataDir)
                    .write(to: launchAgentURL, atomically: true, encoding: .utf8)
                if runLaunchctl(["bootstrap", "gui/\(uid)", launchAgentURL.path]) {
                    trayLog("launch at login enabled, daemon path: \(daemonPath)")
                } else {
                    // A failed bootstrap may be caused by a stale old instance;
                    // bootout first and retry once.
                    trayLog("launchctl bootstrap failed, retrying after bootout")
                    runLaunchctl(["bootout", "gui/\(uid)/com.removent.daemon"])
                    if runLaunchctl(["bootstrap", "gui/\(uid)", launchAgentURL.path]) {
                        trayLog("launch at login enabled (retry succeeded), daemon path: \(daemonPath)")
                    } else {
                        // Still failing: remove the plist just written so the
                        // menu checkbox matches the actual state.
                        try? FileManager.default.removeItem(at: launchAgentURL)
                        trayLog("launchctl bootstrap retry still failed, removed \(launchAgentURL.path)")
                    }
                }
            } catch {
                trayLog("failed to write LaunchAgent: \(error.localizedDescription)")
            }
        }
        rebuildMenu()
    }

    @discardableResult
    private func runLaunchctl(_ arguments: [String]) -> Bool {
        let task = Process()
        task.executableURL = URL(fileURLWithPath: "/bin/launchctl")
        task.arguments = arguments
        task.standardOutput = FileHandle.nullDevice
        task.standardError = FileHandle.nullDevice
        do {
            try task.run()
            task.waitUntilExit()
            return task.terminationStatus == 0
        } catch {
            return false
        }
    }
}
