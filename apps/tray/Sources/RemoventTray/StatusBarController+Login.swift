import AppKit
import Foundation

extension StatusBarController {
    @objc func toggleLaunchAtLogin() {
        runServiceCommand(loginEnabled ? "login-off" : "login-on")
    }

    @objc func restartService() {
        runServiceCommand("restart")
    }

    @objc func toggleProcess() {
        runServiceCommand(connected ? "stop" : "start")
    }

    @objc func setupPermissions() {
        client.requestPermissions()
    }

    // The tray is optional at login. launchd opens it once via Launch Services;
    // quitting it never causes a respawn and never stops the server.
    var trayLoginURL: URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/LaunchAgents/com.alkinum.removent.tray.plist")
    }

    var ownsTrayLoginItem: Bool {
        canManageTrayLogin(bundleID: Bundle.main.bundleIdentifier,
                           dataDirectory: DaemonClient.dataDirectory(),
                           home: FileManager.default.homeDirectoryForCurrentUser)
    }

    static func writeTrayLoginItem(to url: URL, bundle: URL) throws {
        let plist: [String: Any] = [
            "Label": "com.alkinum.removent.tray",
            "ProgramArguments": ["/usr/bin/open", "-g", "-n", "--env",
                                 "REMOVENT_DATA_DIR=\(DaemonClient.dataDirectory().path)", bundle.path],
            "RunAtLoad": true,
            "LimitLoadToSessionType": "Aqua"
        ]
        let data = try PropertyListSerialization.data(fromPropertyList: plist, format: .xml, options: 0)
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try data.write(to: url, options: .atomic)
    }

    @objc func toggleTrayAtLogin() {
        guard ownsTrayLoginItem else { return }
        objectWillChange.send()
        do {
            if FileManager.default.fileExists(atPath: trayLoginURL.path) {
                try FileManager.default.removeItem(at: trayLoginURL)
            } else {
                try Self.writeTrayLoginItem(to: trayLoginURL, bundle: Bundle.main.bundleURL)
            }
        } catch { showServiceError(error.localizedDescription) }
        rebuildMenu()
    }
}
