import AppKit
import Foundation

extension StatusBarController {
    func handle(_ message: DaemonMessage) {
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
            showServiceError(message)
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
            trayLog("pairing PIN received")
            pendingPin = pin
            rebuildMenu()
            showPairingAlert(pin: pin)
        case .pairingCleared:
            pendingPin = nil
            dismissPairingAlert()
            rebuildMenu()
        case .pairingDone(let peerName):
            trayLog("pairing completed: \(peerName)")
            pendingPin = nil
            dismissPairingAlert()
            rebuildMenu()
        case .unknown(let type):
            trayLog("ignoring unknown message type: \(type)")
        }
    }

    // MARK: - Alerts

    /// Whether the main app (GPUI window) is running: while running, admission
    /// and pairing alerts are preferably handled by the main app.
}
