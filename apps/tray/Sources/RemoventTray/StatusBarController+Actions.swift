import AppKit
import Foundation

extension StatusBarController {
    @objc func toggleService() {
        let target = !(status?.running ?? false)
        trayLog("requesting service \(target ? "enable" : "disable")")
        if connected {
            client.setEnabled(target)
        } else {
            enableOnConnect = true
            runServiceCommand("start") { [weak self] success in
                guard let self else { return }
                if !success { self.enableOnConnect = false }
                if success && self.connected {
                    self.enableOnConnect = false
                    self.client.setEnabled(true)
                }
            }
        }
    }

    @objc func openMainApp() {
        if let path = ProcessInfo.processInfo.environment["REMOVENT_DEV_APP"] {
            if let app = NSWorkspace.shared.runningApplications.first(where: { $0.executableURL?.path == path }) {
                app.activate(options: [.activateAllWindows])
                return
            }
            let process = Process()
            process.executableURL = URL(fileURLWithPath: path)
            process.environment = ProcessInfo.processInfo.environment
            process.standardInput = FileHandle.nullDevice
            process.standardOutput = FileHandle.nullDevice
            process.standardError = FileHandle.nullDevice
            do {
                try process.run()
            } catch {
                showOpenMainAppError(error)
            }
            return
        }
        // Reopen the app containing this helper, even if Launch Services knows
        // another installed copy. Keep the same data root across desktop restarts.
        let embedded = outerBundle.pathExtension == "app"
            && FileManager.default.isExecutableFile(atPath: outerBundle.appendingPathComponent("Contents/MacOS/removent").path)
        guard let url = embedded ? outerBundle : NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.alkinum.removent") else {
            trayLog("main app not installed (bundle id com.alkinum.removent not found)")
            showErrorAlert(messageText: String(localized: "alert.main_app_not_found", bundle: .trayResources, comment: "Error alert: main app not installed"),
                           informativeText: String(localized: "alert.main_app_not_found_detail", bundle: .trayResources, comment: "Error alert body: main app not installed"))
            return
        }
        let configuration = NSWorkspace.OpenConfiguration()
        configuration.environment = [
            "REMOVENT_DATA_DIR": DaemonClient.dataDirectory().path,
        ]
        NSWorkspace.shared.openApplication(at: url, configuration: configuration) { [weak self] _, error in
            guard let error else { return }
            DispatchQueue.main.async {
                self?.showOpenMainAppError(error)
            }
        }
    }

    func showOpenMainAppError(_ error: Error) {
        trayLog("failed to open main app: \(error.localizedDescription)")
        showErrorAlert(messageText: String(localized: "alert.main_app_not_found", bundle: .trayResources, comment: "Error alert: main app not installed"),
                       informativeText: String(format: String(localized: "alert.open_main_app_failed", bundle: .trayResources, comment: "Error alert body: failed to open main app"), error.localizedDescription))
    }

    @objc func openDataDirectory() {
        let url = DaemonClient.dataDirectory()
        try? FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        NSWorkspace.shared.open(url)
    }

    @objc func quitTray() {
        trayLog("quitting menu bar app (daemon unaffected)")
        client.stop()
        NSApp.terminate(nil)
    }

    // MARK: - Background service (shared with the app and CLI)

}
