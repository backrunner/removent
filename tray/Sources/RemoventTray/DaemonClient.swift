import Foundation

func trayLog(_ message: String) {
    print("[RemoventTray] \(message)")
    fflush(stdout)
}

// MARK: - Protocol models

struct Session: Codable, Equatable {
    let id: Int
    let peer_name: String
    let peer_fp16: String
    let since_unix: Int
    let video_codec: String
}

struct StatusResponse: Codable, Equatable {
    let running: Bool
    let port: Int
    let device_name: String
    let fp_short: String
    let sessions: [Session]
    let pending_pin: String?
    let tray_connected: Bool
    let screen_recording_granted: Bool
    let accessibility_granted: Bool
}

enum DaemonMessage {
    case status(StatusResponse)
    case ok
    case error(String)
    case stateChanged(running: Bool)
    case sessionStarted(Session)
    case sessionEnded(id: Int, reason: String)
    case admissionRequest(requestId: Int, peerName: String, peerFp16: String)
    case admissionResolved(requestId: Int, allow: Bool)
    case pairingPin(String)
    case pairingDone(peerName: String)
    case unknown(String)
}

// MARK: - Daemon IPC client

/// POSIX AF_UNIX socket client; reads and writes on a background thread and
/// dispatches all callbacks to the main thread. Auto-reconnects on disconnect
/// (1s backoff).
final class DaemonClient {

    static func dataDirectory() -> URL {
        if let env = ProcessInfo.processInfo.environment["REMOVENT_DATA_DIR"], !env.isEmpty {
            return URL(fileURLWithPath: (env as NSString).expandingTildeInPath)
        }
        return FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Application Support/removent/userdata")
    }

    var socketPath: String {
        Self.dataDirectory().appendingPathComponent("run/removentd.sock").path
    }

    /// Both are invoked on the main thread.
    var onMessage: ((DaemonMessage) -> Void)?
    var onConnectionChange: ((Bool) -> Void)?

    private var fd: Int32 = -1
    private let writeLock = NSLock()
    private let queue = DispatchQueue(label: "com.removent.tray.daemon-client")
    /// Serial write queue: preserves write order and keeps the write loop from
    /// blocking the caller (usually the main thread).
    private let writeQueue = DispatchQueue(label: "com.removent.tray.daemon-client.write")
    private var running = false

    func start() {
        guard !running else { return }
        running = true
        queue.async { self.connectLoop() }
    }

    func stop() {
        running = false
        writeLock.lock()
        if fd >= 0 { shutdown(fd, SHUT_RDWR) }
        writeLock.unlock()
    }

    // MARK: Requests

    func requestStatus() { send(["type": "status"]) }
    func setEnabled(_ on: Bool) { send(["type": "set_enabled", "on": on]) }
    func requestPermissions() { send(["type": "request_permissions"]) }
    func admissionReply(requestId: Int, allow: Bool) {
        send(["type": "admission_reply", "request_id": requestId, "allow": allow])
    }
    func kickSession(_ sessionId: Int) { send(["type": "kick_session", "session_id": sessionId]) }
    func requestShutdown() { send(["type": "shutdown"]) }

    // MARK: Connect and read

    private func connectLoop() {
        while running {
            let newFd = connectUnix(path: socketPath)
            guard newFd >= 0 else {
                notifyConnection(false)
                Thread.sleep(forTimeInterval: 1)
                continue
            }
            writeLock.lock()
            fd = newFd
            writeLock.unlock()
            trayLog("connected to daemon: \(socketPath)")
            notifyConnection(true)
            readLoop(fd: newFd)
            writeLock.lock()
            if fd == newFd { fd = -1 }
            writeLock.unlock()
            close(newFd)
            trayLog("daemon connection lost, reconnecting in 1s")
            notifyConnection(false)
            Thread.sleep(forTimeInterval: 1)
        }
    }

    private func connectUnix(path: String) -> Int32 {
        let newFd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard newFd >= 0 else { return -1 }
        // Suppress SIGPIPE when writing to a closed socket (a daemon-death
        // race would otherwise kill the whole tray process).
        var one: Int32 = 1
        setsockopt(newFd, SOL_SOCKET, SO_NOSIGPIPE, &one, socklen_t(MemoryLayout<Int32>.size))
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let capacity = MemoryLayout.size(ofValue: addr.sun_path)
        guard path.utf8.count < capacity else {
            close(newFd)
            return -1
        }
        _ = withUnsafeMutablePointer(to: &addr.sun_path) { pathPtr in
            pathPtr.withMemoryRebound(to: CChar.self, capacity: capacity) { dest in
                path.withCString { strncpy(dest, $0, capacity - 1) }
            }
        }
        let len = socklen_t(MemoryLayout<sockaddr_un>.size)
        let result = withUnsafePointer(to: &addr) { ptr in
            ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                connect(newFd, $0, len)
            }
        }
        if result != 0 {
            close(newFd)
            return -1
        }
        return newFd
    }

    private func readLoop(fd: Int32) {
        var buffer = Data()
        var chunk = [UInt8](repeating: 0, count: 65536)
        while running {
            let n = recv(fd, &chunk, chunk.count, 0)
            if n <= 0 { return }
            buffer.append(contentsOf: chunk[0..<n])
            while let nl = buffer.firstIndex(of: 0x0A) {
                let line = Data(buffer.prefix(nl))
                buffer = Data(buffer.suffix(from: buffer.index(after: nl)))
                if !line.isEmpty { handleLine(line) }
            }
        }
    }

    // MARK: Message dispatch

    private func handleLine(_ data: Data) {
        guard let obj = try? JSONSerialization.jsonObject(with: data),
              let dict = obj as? [String: Any],
              let type = dict["type"] as? String else {
            return
        }
        let message: DaemonMessage
        switch type {
        case "status":
            guard let status = try? JSONDecoder().decode(StatusResponse.self, from: data) else { return }
            message = .status(status)
        case "ok":
            message = .ok
        case "error":
            message = .error(dict["message"] as? String ?? "unknown error")
        case "state_changed":
            message = .stateChanged(running: dict["running"] as? Bool ?? false)
        case "session_started":
            guard let raw = dict["session"],
                  let rawData = try? JSONSerialization.data(withJSONObject: raw),
                  let session = try? JSONDecoder().decode(Session.self, from: rawData) else { return }
            message = .sessionStarted(session)
        case "session_ended":
            message = .sessionEnded(id: dict["session_id"] as? Int ?? -1,
                                    reason: dict["reason"] as? String ?? "")
        case "admission_request":
            message = .admissionRequest(requestId: dict["request_id"] as? Int ?? -1,
                                        peerName: dict["peer_name"] as? String ?? "unknown device",
                                        peerFp16: dict["peer_fp16"] as? String ?? "")
        case "admission_resolved":
            message = .admissionResolved(requestId: dict["request_id"] as? Int ?? -1,
                                         allow: dict["allow"] as? Bool ?? false)
        case "pairing_pin":
            message = .pairingPin(dict["pin"] as? String ?? "")
        case "pairing_done":
            message = .pairingDone(peerName: dict["peer_name"] as? String ?? "")
        default:
            message = .unknown(type)
        }
        DispatchQueue.main.async { [weak self] in
            self?.onMessage?(message)
        }
    }

    private func notifyConnection(_ connected: Bool) {
        DispatchQueue.main.async { [weak self] in
            self?.onConnectionChange?(connected)
        }
    }

    // MARK: Write

    private func send(_ dict: [String: Any]) {
        guard var data = try? JSONSerialization.data(withJSONObject: dict) else { return }
        data.append(0x0A)
        // Dispatch to the serial write queue: the write loop may block and
        // must not run on the main thread; the serial queue preserves write
        // order while writeLock keeps protecting fd access.
        writeQueue.async { [weak self] in
            guard let self else { return }
            self.writeLock.lock()
            defer { self.writeLock.unlock() }
            guard self.fd >= 0 else { return }
            data.withUnsafeBytes { ptr in
                guard let base = ptr.baseAddress else { return }
                var offset = 0
                while offset < data.count {
                    let n = Darwin.write(self.fd, base.advanced(by: offset), data.count - offset)
                    if n <= 0 { return }
                    offset += n
                }
            }
        }
    }
}
