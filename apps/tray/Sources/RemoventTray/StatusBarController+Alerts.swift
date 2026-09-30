import AppKit
import Foundation

extension StatusBarController {
    var mainAppRunning: Bool {
        if let path = ProcessInfo.processInfo.environment["REMOVENT_DEV_APP"] {
            return NSWorkspace.shared.runningApplications.contains { $0.executableURL?.path == path }
        }
        return !NSRunningApplication.runningApplications(withBundleIdentifier: "com.alkinum.removent").isEmpty
    }

    /// Queue admission requests that arrive while an alert is being shown
    /// (the main thread still dispatches events inside runModal) to avoid
    /// nested modals.
    func enqueueOrShowAdmissionAlert(requestId: Int, peerName: String, peerFp16: String) {
        if alertActive {
            queuedAdmissionRequests.append((requestId, peerName, peerFp16))
            trayLog("alert already active, admission request #\(requestId) queued")
            return
        }
        showAdmissionAlert(requestId: requestId, peerName: peerName, peerFp16: peerFp16)
    }

    func processQueuedAdmissionAlerts() {
        guard !alertActive, !queuedAdmissionRequests.isEmpty else { return }
        let next = queuedAdmissionRequests.removeFirst()
        showAdmissionAlert(requestId: next.requestId, peerName: next.peerName, peerFp16: next.peerFp16)
    }

    /// Show an admission alert directly (the caller has confirmed the tray
    /// should handle it); sets alertActive for the duration.
    func showAdmissionAlert(requestId: Int, peerName: String, peerFp16: String) {
        let alert = NSAlert()
        alert.messageText = String(format: String(localized: "alert.connection_request", bundle: .trayResources, comment: "Admission alert title"), peerName)
        alert.informativeText = String(format: String(localized: "alert.connection_request_detail", bundle: .trayResources, comment: "Admission alert body"), peerFp16)
        alert.addButton(withTitle: String(localized: "alert.allow", bundle: .trayResources, comment: "Admission alert allow button"))
        alert.addButton(withTitle: String(localized: "alert.deny", bundle: .trayResources, comment: "Admission alert deny button"))
        alert.alertStyle = .warning
        alertActive = true
        NSApp.activate(ignoringOtherApps: true)
        let allow = alert.runModal() == .alertFirstButtonReturn
        alertActive = false
        client.admissionReply(requestId: requestId, allow: allow)
        processQueuedAdmissionAlerts()
    }

    func showPairingAlert(pin: String) {
        if suppressAlerts { return }
        if mainAppRunning {
            trayLog("main app is running; pairing PIN \(pin) handled by main window, tray shows no alert")
            return
        }
        let alert = NSAlert()
        alert.messageText = String(localized: "alert.pairing_request", bundle: .trayResources, comment: "Pairing alert title")
        alert.informativeText = String(format: String(localized: "alert.pairing_pin_detail", bundle: .trayResources, comment: "Pairing alert body"), pin)
        alert.addButton(withTitle: String(localized: "alert.ok", bundle: .trayResources, comment: "Alert OK button"))
        alertActive = true
        NSApp.activate(ignoringOtherApps: true)
        alert.runModal()
        alertActive = false
        processQueuedAdmissionAlerts()
    }

    /// Error alert; in test mode only logs.
    func showErrorAlert(messageText: String, informativeText: String) {
        if suppressAlerts { return }
        let alert = NSAlert()
        alert.messageText = messageText
        alert.informativeText = informativeText
        alert.addButton(withTitle: String(localized: "alert.ok", bundle: .trayResources, comment: "Alert OK button"))
        alert.alertStyle = .warning
        NSApp.activate(ignoringOtherApps: true)
        alert.runModal()
    }

    // MARK: - Menu
}
