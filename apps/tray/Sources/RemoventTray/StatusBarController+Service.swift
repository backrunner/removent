import AppKit
import Foundation

extension StatusBarController {
    var outerBundle: URL {
        Bundle.main.bundleURL.deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
    }

    var serviceCLI: URL? {
        if Bundle.main.bundleURL.pathExtension != "app",
           let path = ProcessInfo.processInfo.environment["REMOVENT_SERVICE_CLI"],
           FileManager.default.isExecutableFile(atPath: path) {
            return URL(fileURLWithPath: path)
        }
        let url = outerBundle.appendingPathComponent("Contents/MacOS/removent-cli")
        return FileManager.default.isExecutableFile(atPath: url.path) ? url : nil
    }

    func showServiceError(_ message: String) {
        showErrorAlert(messageText: String(localized: "alert.service_failed", bundle: .trayResources), informativeText: message)
    }

    func refreshServiceStatus() {
        guard !serviceBusy, !serviceQueryBusy, serviceCLI != nil else { return }
        runServiceCommand("service-status", reportError: false)
    }

    func watchService() {
        guard !connected, !serviceBusy, serviceCLI != nil,
              ProcessInfo.processInfo.environment["REMOVENT_DEV_SUPERVISED"] != "1",
              Date() >= nextRecovery else { return }
        // The shared manager atomically checks user intent before recovering.
        // launchd also watches the process while this UI is closed.
        nextRecovery = Date().addingTimeInterval(recoveryDelay)
        if stoppedByUser {
            refreshServiceStatus()
            return
        }
        runServiceCommand("ensure", reportError: false) { [weak self] success in
            guard let self else { return }
            self.recoveryDelay = success ? 2 : min(30, self.recoveryDelay * 2)
            self.nextRecovery = Date().addingTimeInterval(self.recoveryDelay)
        }
    }

    /// Process waits stay off the UI thread. All service mutations go through
    /// the packaged CLI; the tray never spawns a second daemon or guesses paths.
    func runServiceCommand(_ action: String, reportError: Bool = true, completion: ((Bool) -> Void)? = nil) {
        let query = action == "service-status"
        guard !serviceBusy, !query || !serviceQueryBusy else { completion?(false); return }
        guard let cli = serviceCLI else {
            showServiceError(String(localized: "alert.daemon_not_found_detail", bundle: .trayResources))
            completion?(false)
            return
        }
        // Opening the menu refreshes status. A read must not disable every
        // action for the entire time that menu is open. Ignore a stale query
        // response if a user command has since changed service intent.
        if query {
            serviceQueryBusy = true
        } else {
            serviceBusy = true
            serviceGeneration += 1
            rebuildMenu()
        }
        let generation = serviceGeneration
        DispatchQueue.global(qos: .userInitiated).async {
            let task = Process()
            let output = Pipe()
            task.executableURL = cli
            task.arguments = ["daemon", action]
            var environment = ProcessInfo.processInfo.environment
            environment["REMOVENT_DATA_DIR"] = DaemonClient.dataDirectory().path
            task.environment = environment
            task.standardOutput = output
            task.standardError = output
            var success = false
            var message = ""
            do {
                try task.run()
                let data = output.fileHandleForReading.readDataToEndOfFile()
                task.waitUntilExit()
                success = task.terminationStatus == 0
                message = String(decoding: data, as: UTF8.self)
            } catch { message = error.localizedDescription }
            let result = success
            let detail = message
            DispatchQueue.main.async {
                if query {
                    self.serviceQueryBusy = false
                    guard generation == self.serviceGeneration else { return }
                } else {
                    self.serviceBusy = false
                }
                if result, let data = detail.data(using: .utf8),
                   let status = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
                    self.loginEnabled = status["launch_at_login"] as? Bool ?? false
                    self.stoppedByUser = status["stopped_by_user"] as? Bool ?? false
                    self.recoveryError = nil
                } else if !result {
                    self.recoveryError = detail.trimmingCharacters(in: .whitespacesAndNewlines)
                    trayLog("service \(action) failed: \(detail)")
                    if reportError { self.showServiceError(detail) }
                }
                completion?(result)
                self.rebuildMenu()
            }
        }
    }

}
