import AppKit
import Foundation

extension StatusBarController {
    func updateIcon() {
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

    func menuNeedsUpdate(_ menu: NSMenu) {
        refreshServiceStatus()
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

    func rebuildMenu() {
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
        if !connected && stoppedByUser {
            stateText = String(localized: "status.process_stopped", bundle: .trayResources)
            stateSymbol = "pause.circle"
        } else if !connected && serviceBusy {
            stateText = String(localized: "status.starting", bundle: .trayResources)
            stateSymbol = "arrow.clockwise"
        } else if !connected {
            stateText = String(localized: "status.daemon_offline", bundle: .trayResources, comment: "Menu status: daemon not connected")
            stateSymbol = "exclamationmark.circle"
        } else if status?.running == true && status?.host_ready == false {
            stateText = String(localized: "status.starting", bundle: .trayResources)
            stateSymbol = "exclamationmark.circle"
        } else if status?.running == true {
            stateText = String(localized: "status.running", bundle: .trayResources, comment: "Menu status: service running")
            stateSymbol = "checkmark.circle.fill"
        } else {
            stateText = String(localized: "status.stopped", bundle: .trayResources, comment: "Menu status: service stopped")
            stateSymbol = "pause.circle"
        }
        menu.addItem(infoItem(String(format: String(localized: "menu.service_status", bundle: .trayResources, comment: "Menu status line"), stateText), symbol: stateSymbol))
        if let error = recoveryError {
            menu.addItem(infoItem(error, symbol: "exclamationmark.triangle"))
        }
        if let error = status?.host_error {
            menu.addItem(infoItem(error, symbol: "exclamationmark.triangle"))
        }
        if let relay = status?.relay_connected {
            let key = relay ? "menu.relay_online" : "menu.relay_offline"
            menu.addItem(infoItem(NSLocalizedString(key, bundle: .trayResources, comment: "Relay connection status"), symbol: relay ? "network" : "exclamationmark.triangle"))
            if let error = status?.relay_error { menu.addItem(infoItem(error)) }
        }
        if let s = status, !s.fp_short.isEmpty {
            menu.addItem(infoItem(String(format: String(localized: "menu.device_fingerprint", bundle: .trayResources, comment: "Menu device fingerprint line"), s.fp_short)))
        }
        // Permission warnings (only relevant while hosting).
        if connected, let s = status, s.running {
            if s.screen_recording_granted == false {
                menu.addItem(infoItem(String(localized: "menu.permission_screen_recording_missing", bundle: .trayResources, comment: "Menu warning: screen recording permission missing"), symbol: "exclamationmark.triangle"))
            }
            if s.accessibility_granted == false {
                menu.addItem(infoItem(String(localized: "menu.permission_accessibility_missing", bundle: .trayResources, comment: "Menu warning: accessibility permission missing"), symbol: "exclamationmark.triangle"))
            }
        }
        menu.addItem(.separator())

        // 1.5 Open main window
        let mainItem = NSMenuItem(title: String(localized: "menu.open_main_window", bundle: .trayResources, comment: "Menu item: open main window"), action: #selector(openMainApp), keyEquivalent: "")
        mainItem.target = self
        mainItem.image = symbolImage("macwindow")
        menu.addItem(mainItem)
        menu.addItem(.separator())

        // 2. Sessions
        let sessions = connected ? (status?.sessions ?? []) : []
        if sessions.isEmpty {
            menu.addItem(infoItem(String(localized: "menu.no_sessions", bundle: .trayResources, comment: "Menu item: no active sessions")))
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
            item.attributedTitle = NSAttributedString(string: String(format: String(localized: "menu.pairing_pin", bundle: .trayResources, comment: "Menu pairing PIN line"), pin), attributes: attrs)
            item.isEnabled = false
            menu.addItem(item)
            menu.addItem(.separator())
        }

        // 4. Enable/disable service
        let toggleTitle = (status?.running == true)
            ? String(localized: "menu.disable_service", bundle: .trayResources, comment: "Menu item: disable service")
            : String(localized: "menu.enable_service", bundle: .trayResources, comment: "Menu item: enable service")
        let toggleItem = NSMenuItem(title: toggleTitle, action: #selector(toggleService), keyEquivalent: "")
        toggleItem.target = self
        toggleItem.isEnabled = !serviceBusy
        toggleItem.image = symbolImage("power")
        menu.addItem(toggleItem)

        // 5. Launch at login
        let loginItem = NSMenuItem(title: String(localized: "menu.launch_at_login", bundle: .trayResources, comment: "Menu item: launch at login"), action: #selector(toggleLaunchAtLogin), keyEquivalent: "")
        loginItem.target = self
        loginItem.state = loginEnabled ? .on : .off
        loginItem.isEnabled = !serviceBusy && serviceCLI != nil
        loginItem.image = symbolImage("arrow.up.circle")
        menu.addItem(loginItem)

        let restartItem = NSMenuItem(title: String(localized: "menu.restart_service", bundle: .trayResources), action: #selector(restartService), keyEquivalent: "")
        restartItem.target = self
        restartItem.isEnabled = !serviceBusy && (status?.sessions.isEmpty ?? true) && serviceCLI != nil
        menu.addItem(restartItem)

        let processItem = NSMenuItem(title: NSLocalizedString(connected ? "menu.stop_process" : "menu.start_process", bundle: .trayResources, comment: "Background process control"), action: #selector(toggleProcess), keyEquivalent: "")
        processItem.target = self
        processItem.isEnabled = !serviceBusy && (status?.sessions.isEmpty ?? true) && serviceCLI != nil
        menu.addItem(processItem)

        let permissionsItem = NSMenuItem(title: String(localized: "menu.setup_permissions", bundle: .trayResources), action: #selector(setupPermissions), keyEquivalent: "")
        permissionsItem.target = self
        permissionsItem.isEnabled = connected
        menu.addItem(permissionsItem)

        let trayLogin = NSMenuItem(title: String(localized: "menu.tray_at_login", bundle: .trayResources), action: #selector(toggleTrayAtLogin), keyEquivalent: "")
        trayLogin.target = self
        trayLogin.state = ownsTrayLoginItem && FileManager.default.fileExists(atPath: trayLoginURL.path) ? .on : .off
        trayLogin.isEnabled = ownsTrayLoginItem
        menu.addItem(trayLogin)

        menu.addItem(infoItem(String(localized: "menu.unattended_hint", bundle: .trayResources)))
        if FileManager.default.fileExists(atPath: "/Library/LaunchAgents/com.alkinum.removent.loginwindow.plist") {
            menu.addItem(infoItem(String(localized: "menu.loginwindow_managed", bundle: .trayResources), symbol: "lock.shield"))
        }

        menu.addItem(.separator())

        // 6. Open data directory
        let openItem = NSMenuItem(title: String(localized: "menu.open_data_directory", bundle: .trayResources, comment: "Menu item: open data directory"), action: #selector(openDataDirectory), keyEquivalent: "")
        openItem.target = self
        openItem.image = symbolImage("folder")
        menu.addItem(openItem)

        // 7. Quit menu bar app
        let quitItem = NSMenuItem(title: String(localized: "menu.quit", bundle: .trayResources, comment: "Menu item: quit tray"), action: #selector(quitTray), keyEquivalent: "q")
        quitItem.target = self
        quitItem.image = symbolImage("xmark.circle")
        menu.addItem(quitItem)
    }

    func infoItem(_ title: String, symbol: String? = nil) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: nil, keyEquivalent: "")
        item.isEnabled = false
        if let symbol {
            item.image = symbolImage(symbol)
        }
        return item
    }

    func symbolImage(_ name: String) -> NSImage? {
        let image = NSImage(systemSymbolName: name, accessibilityDescription: nil)
        image?.isTemplate = true
        return image
    }

    func durationString(since unix: Int) -> String {
        let secs = max(0, Int(Date().timeIntervalSince1970) - unix)
        let h = secs / 3600, m = (secs % 3600) / 60, s = secs % 60
        if h > 0 { return String(format: String(localized: "session.duration.hm", bundle: .trayResources, comment: "Session duration: hours and minutes"), h, m) }
        if m > 0 { return String(format: String(localized: "session.duration.ms", bundle: .trayResources, comment: "Session duration: minutes and seconds"), m, s) }
        return String(format: String(localized: "session.duration.s", bundle: .trayResources, comment: "Session duration: seconds"), s)
    }

    // MARK: - Actions

}
