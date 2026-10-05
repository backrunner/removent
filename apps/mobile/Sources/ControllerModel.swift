import SwiftUI
import AVFoundation

struct CertificatePrompt: Identifiable, Equatable {
    let id = UUID()
    let destination: String
    let relay: Bool
}

@MainActor
final class ControllerModel: ObservableObject {
    @Published var cloudSync: ConnectionSyncService?
    @Published var bookmarks: [Bookmark] = []
    @Published var presentingSession = false
    @Published var ready = false
    @Published var authenticationMode = "pairing_code"
    @Published var certificatePrompt: CertificatePrompt?
    var certificateDestination: String? { certificatePrompt?.destination }
    var confirmingRelay: Bool { certificatePrompt?.relay ?? false }
    @Published var needsPIN = false
    @Published var isConnecting = false
    @Published var status = ""
    @Published var error: String?
    @Published var sessionTitle = ""
    @Published var codec = ""
    @Published var remoteClipboard = false
    @Published var remoteAudio = false
    @Published var canRefresh = false
    @Published var frameSize = CGSize.zero
    @Published var fps = 0
    @Published var receivedFrames = 0
    @Published var scanDevices: [NearbyDevice] = []
    @Published var fingerprint = ""
    @Published var muted = false { didSet { audio.muted = muted } }
    @Published var audioEnabled = UserDefaults.standard.object(forKey: "audioEnabled") as? Bool ?? true {
        didSet { UserDefaults.standard.set(audioEnabled, forKey: "audioEnabled") }
    }
    @Published var clipboardEnabled = UserDefaults.standard.object(forKey: "clipboardEnabled") as? Bool ?? true {
        didSet { UserDefaults.standard.set(clipboardEnabled, forKey: "clipboardEnabled") }
    }
    @Published var discoverRemovent = UserDefaults.standard.object(forKey: "discoverRemovent") as? Bool ?? true {
        didSet { UserDefaults.standard.set(discoverRemovent, forKey: "discoverRemovent"); if servicesStarted { startDiscovery() } }
    }
    @Published var discoverVNC = UserDefaults.standard.object(forKey: "discoverVNC") as? Bool ?? false {
        didSet { UserDefaults.standard.set(discoverVNC, forKey: "discoverVNC"); if servicesStarted { startDiscovery() } }
    }
    @Published var discoverRDP = UserDefaults.standard.object(forKey: "discoverRDP") as? Bool ?? false {
        didSet { UserDefaults.standard.set(discoverRDP, forKey: "discoverRDP"); if servicesStarted { startDiscovery() } }
    }
    let discovery = BonjourDiscovery()
    var onFrame: ((CGImage) -> Void)?
    private var servicesStarted = false
    private var handle: OpaquePointer?
    private var generation: UInt64 = 0
    private var timer: Timer?
    private var retryCommand: [String: Any]?
    private var pendingInvitation: ConnectionDraft?
    private var sampleTime = CACurrentMediaTime()
    private var sampleFrames = 0
    private let audio = RemoteAudio()

    init(storageDirectory: URL? = nil, startServices: Bool = true) {
        do {
            var directory = try storageDirectory ?? FileManager.default.url(for: .applicationSupportDirectory,
                in: .userDomainMask, appropriateFor: nil, create: true).appendingPathComponent("Removent", isDirectory: true)
            #if DEBUG
            if ProcessInfo.processInfo.arguments.contains("--ui-testing") {
                directory = directory.appendingPathComponent("UITests", isDirectory: true)
                if ProcessInfo.processInfo.arguments.contains("--ui-testing-fresh") {
                    directory = directory.appendingPathComponent(UUID().uuidString, isDirectory: true)
                }
            }
            #endif
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true,
                attributes: [.protectionKey:FileProtectionType.completeUntilFirstUserAuthentication])
            var values = URLResourceValues(); values.isExcludedFromBackup = true
            try directory.setResourceValues(values)
            var failure: UnsafeMutablePointer<CChar>?
            handle = rm_create(directory.path, UIDevice.current.name, &failure)
            if let failure { defer { rm_string_free(failure) }; throw MobileError.message(String(cString:failure)) }
            guard handle != nil else { throw MobileError.message("Controller unavailable") }
            try reload()
            let storagePath = directory.path
            cloudSync = ConnectionSyncService { command in
                let data = try JSONSerialization.data(withJSONObject: command)
                let input = String(decoding: data, as: UTF8.self)
                let response = try await Task.detached(priority: .utility) {
                    guard let result = rm_sync(storagePath, input) else { throw MobileError.message("Sync storage unavailable") }
                    defer { rm_string_free(result) }
                    return Data(String(cString: result).utf8)
                }.value
                guard let result = try JSONSerialization.jsonObject(with: response) as? [String: Any],
                      result["ok"] as? Bool == true, let value = result["value"] as? [String: Any] else {
                    throw MobileError.message("Could not update connection storage")
                }
                return value
            }
            cloudSync?.didChange = { [weak self] in
                do { try self?.reload() } catch { self?.error = error.localizedDescription }
            }
            guard startServices else { return }
            servicesStarted = true
            cloudSync?.start()
            timer = Timer(timeInterval: 1.0 / 60.0, repeats: true) { [weak self] _ in
                MainActor.assumeIsolated { self?.poll() }
            }
            RunLoop.main.add(timer!, forMode: .common)
            #if DEBUG
            if !ProcessInfo.processInfo.arguments.contains("--ui-testing") { startDiscovery() }
            #else
            startDiscovery()
            #endif
        } catch { self.error = error.localizedDescription }
    }

    deinit { timer?.invalidate(); if let handle { rm_destroy(handle) } }

    @discardableResult
    func call(_ command: [String: Any]) throws -> Any {
        guard let handle else { throw MobileError.message("Controller unavailable") }
        let data = try JSONSerialization.data(withJSONObject: command)
        guard let input = String(data:data, encoding:.utf8), let output = rm_call(handle, input) else {
            throw MobileError.message("Controller returned no result")
        }
        defer { rm_string_free(output) }
        let result = try JSONSerialization.jsonObject(with:Data(String(cString:output).utf8)) as? [String:Any]
        guard result?["ok"] as? Bool == true else {
            throw MobileError.message(result?["error"] as? String ?? "Controller failed")
        }
        return result?["value"] ?? NSNull()
    }

    func reload() throws {
        guard let result = try call(["op":"list"]) as? [String:Any], let connections = result["connections"] else {
            throw MobileError.message("Controller returned an invalid connection list")
        }
        bookmarks = try JSONDecoder().decode([Bookmark].self,
            from:JSONSerialization.data(withJSONObject:connections))
        fingerprint = result["fingerprint"] as? String ?? ""
    }
    func save(_ draft: ConnectionDraft) throws {
        var command: [String:Any] = ["op":"save", "name":draft.name,
            "request":try draft.request(audio:audioEnabled, clipboard:clipboardEnabled)]
        if let id = draft.bookmarkID { command["id"] = id }
        if let id = draft.credentialID { command["credential_id"] = id }
        try call(command); try reload(); if servicesStarted { cloudSync?.wake() }
    }
    func validate(_ draft: ConnectionDraft) throws {
        var command: [String:Any] = ["op":"validate",
            "request":try draft.request(audio:audioEnabled, clipboard:clipboardEnabled)]
        if let id = draft.credentialID { command["credential_id"] = id }
        try call(command)
    }
    func delete(_ bookmark: Bookmark) {
        do { try call(["op":"delete", "id":bookmark.id]); try reload(); if servicesStarted { cloudSync?.wake() } }
        catch { self.error = error.localizedDescription }
    }
    func connect(_ draft: ConnectionDraft) throws {
        pendingInvitation = draft.isPairingCode && draft.rememberAfterPairing ? draft : nil
        var command: [String:Any] = ["op":"connect", "request":try draft.request(audio:audioEnabled, clipboard:clipboardEnabled)]
        if let id = draft.credentialID { command["credential_id"] = id }
        try begin(command, title:draft.title)
    }
    func connect(_ bookmark: Bookmark) {
        pendingInvitation = nil
        do { try begin(["op":"connect_saved", "id":bookmark.id, "audio":audioEnabled,
            "clipboard":clipboardEnabled], title:bookmark.title) }
        catch { self.error = error.localizedDescription }
    }
    private func begin(_ command: [String:Any], title: String) throws {
        guard let result = try call(command) as? [String:Any], let value = result["generation"] as? NSNumber else {
            throw MobileError.message("Controller returned an invalid session")
        }
        generation = value.uint64Value
        retryCommand = command; sessionTitle = title; error = nil
        ready = false; needsPIN = false; certificatePrompt = nil; isConnecting = true; frameSize = .zero; receivedFrames = 0; fps = 0
        sampleFrames = 0; sampleTime = CACurrentMediaTime()
        codec = ""; remoteClipboard = false; remoteAudio = false; canRefresh = false; audio.stop()
        status = L("Connecting…", "正在连接…"); presentingSession = true
        UIApplication.shared.isIdleTimerDisabled = true
    }
    func retry() {
        guard let retryCommand else { return }
        do { try begin(retryCommand, title:sessionTitle) } catch { self.error = error.localizedDescription }
    }
    func disconnect(dismiss: Bool = true) {
        let result = try? call(["op":"disconnect"]) as? [String:Any]
        generation = (result?["generation"] as? NSNumber)?.uint64Value ?? generation + 1
        ready = false; needsPIN = false; certificatePrompt = nil; isConnecting = false; audio.stop()
        status = L("Disconnected", "连接已断开")
        UIApplication.shared.isIdleTimerDisabled = false
        if dismiss { presentingSession = false; error = nil; retryCommand = nil; pendingInvitation = nil }
    }
    func confirmCertificate(_ accept: Bool, promptID: UUID? = nil) {
        guard let prompt = certificatePrompt, promptID == nil || promptID == prompt.id else { return }
        do {
            try call(["op":"confirm_certificate", "generation":generation, "accept":accept])
            certificatePrompt = nil
            if accept {
                isConnecting = true; status = L("Connecting…", "正在连接…")
            } else { disconnect(dismiss:false) }
        } catch { self.error = error.localizedDescription; certificatePrompt = nil }
    }
    func submitPIN(_ pin: String) {
        do {
            try call(["op":"pin", "generation":generation, "pin":pin]); needsPIN = false
            isConnecting = true; error = nil
            status = authenticationMode == "pairing_code" ? L("Waiting for the computer…", "正在等待电脑确认…") : L("Authenticating…", "正在验证身份…")
        } catch { self.error = error.localizedDescription }
    }
    func input(_ event: [String:Any]) {
        guard ready else { return }
        do { try call(["op":"input", "generation":generation, "event":event]) }
        catch { self.error = error.localizedDescription; disconnect(dismiss:false) }
    }
    func text(_ text: String) {
        // Bound the cross-language input queue. IME commits are usually short.
        if text.unicodeScalars.count > 64 {
            error = L("Use clipboard to send longer text.", "请通过剪贴板发送较长文本。")
        } else { input(["kind":"text", "text":text]) }
    }
    func key(_ code: Int, modifiers: Int = 0) {
        input(["kind":"key", "code":code, "modifiers":modifiers, "down":true])
        input(["kind":"key", "code":code, "modifiers":0, "down":false])
    }
    @discardableResult
    func sendClipboard(_ text: String) -> Bool {
        do { try call(["op":"clipboard", "generation":generation, "text":text]); error = nil; return true }
        catch { self.error = error.localizedDescription; return false }
    }
    @discardableResult
    func copyRemoteClipboard() -> Bool {
        do {
            let result = try call(["op":"read_clipboard", "generation":generation]) as? [String:Any]
            guard let text = result?["text"] as? String else {
                throw MobileError.message(L("The computer has not shared any text yet.", "电脑尚未共享文本。"))
            }
            UIPasteboard.general.string = text; error = nil; return true
        } catch { self.error = error.localizedDescription; return false }
    }
    func refresh() {
        do { try call(["op":"keyframe", "generation":generation]) }
        catch { self.error = error.localizedDescription }
    }
    func startDiscovery() {
        discovery.start(removent:discoverRemovent, vnc:discoverVNC, rdp:discoverRDP)
        scanDevices = []
        do { try call(["op":"scan", "vnc":discoverVNC, "rdp":discoverRDP]) }
        catch { self.error = error.localizedDescription }
    }
    func background() {
        servicesStarted = false
        discovery.stop(); scanDevices = []
        _ = try? call(["op":"scan", "vnc":false, "rdp":false])
        if presentingSession {
            disconnect(dismiss:false)
            status = L("Session paused while in the background.", "进入后台后会话已暂停。")
        }
        timer?.fireDate = .distantFuture
    }
    func foreground() {
        cloudSync?.wake()
        servicesStarted = true
        timer?.fireDate = .now
        #if DEBUG
        if ProcessInfo.processInfo.arguments.contains("--ui-testing") { return }
        #endif
        startDiscovery()
    }

    private func poll() {
        guard let handle else { return }
        for _ in 0..<32 {
            guard let raw = rm_poll_event(handle) else { break }
            let data = Data(String(cString:raw).utf8); rm_string_free(raw)
            if let event = try? JSONSerialization.jsonObject(with:data) as? [String:Any] { accept(event) }
        }
        if let frame = rm_take_frame(handle) {
            let value = frame.pointee
            if value.generation == generation, let pixels = value.data, value.len > 0 {
                let provider = CGDataProvider(dataInfo:UnsafeMutableRawPointer(frame), data:pixels,
                    size:value.len) { info, _, _ in
                        if let info { rm_frame_free(info.assumingMemoryBound(to:RMFrame.self)) }
                    }
                if let provider, let image = CGImage(width:Int(value.width), height:Int(value.height),
                    bitsPerComponent:8, bitsPerPixel:32, bytesPerRow:Int(value.width) * 4,
                    space:CGColorSpaceCreateDeviceRGB(), bitmapInfo:[.byteOrder32Little,
                        CGBitmapInfo(rawValue:CGImageAlphaInfo.noneSkipFirst.rawValue)],
                    provider:provider, decode:nil, shouldInterpolate:true, intent:.defaultIntent) {
                    let size = CGSize(width:Int(value.width), height:Int(value.height))
                    if frameSize != size { frameSize = size }
                    onFrame?(image); sampleFrames += 1
                } else if provider == nil { rm_frame_free(frame) }
            } else { rm_frame_free(frame) }
        }
        for _ in 0..<12 {
            guard let pcm = rm_take_audio(handle) else { break }
            if pcm.pointee.generation == generation { audio.push(pcm.pointee) }
            rm_audio_free(pcm)
        }
        let now = CACurrentMediaTime()
        if now - sampleTime >= 1 {
            fps = Int(Double(sampleFrames) / (now - sampleTime)); receivedFrames += sampleFrames
            sampleFrames = 0; sampleTime = now
        }
    }
    func accept(_ event: [String:Any]) {
        if event["type"] as? String == "scan", let protocolName = event["protocol"] as? String,
            let proto = ConnectionProtocol(rawValue:protocolName), let devices = event["devices"] as? [[String:Any]] {
            guard (proto == .vnc && discoverVNC) || (proto == .rdp && discoverRDP) else { return }
            scanDevices.removeAll { $0.protocol == proto }
            scanDevices += devices.compactMap {
                guard let id = $0["id"] as? String, let name = $0["name"] as? String,
                    let host = $0["host"] as? String, let port = $0["port"] as? Int else { return nil }
                return NearbyDevice(id:id, name:name, host:host, port:port, protocol:proto)
            }
            return
        }
        guard (event["generation"] as? NSNumber)?.uint64Value == generation else { return }
        switch event["type"] as? String {
        case "resolve_pairing":
            guard let locator = event["locator"] as? String else { return }
            let current = generation
            Task { @MainActor [weak self] in
                guard let self else { return }
                do {
                    let device = try await BonjourDiscovery.resolvePairing(locator)
                    guard self.generation == current else { return }
                    try self.call(["op":"pairing_address", "generation":current, "host":device.host, "port":device.port])
                } catch {
                    guard self.generation == current else { return }
                    try? self.call(["op":"pairing_address", "generation":current, "host":"", "port":0, "error":error.localizedDescription])
                }
            }
        case "certificate":
            guard let destination = event["destination"] as? String else { return }
            certificatePrompt = CertificatePrompt(destination:destination, relay:event["relay"] as? Bool ?? false)
            needsPIN = false; isConnecting = false
            status = L("Confirm the first connection", "请确认首次连接")
        case "pin":
            authenticationMode = event["mode"] as? String ?? "pairing_code"
            needsPIN = true; isConnecting = false
            switch authenticationMode {
            case "password": status = L("Enter the password configured on the computer", "输入电脑上设置的访问口令")
            case "otp": status = L("Enter the current code from your authenticator", "输入验证器中的当前动态验证码")
            default: status = L("Enter the PIN shown on the computer", "输入电脑上显示的配对码")
            }
        case "progress":
            let stage = event["stage"] as? String ?? "Connecting"
            let stages = ["Resolving":L("Finding computer…", "正在查找电脑…"),
                "Connecting":L("Connecting…", "正在连接…"), "Negotiating":L("Negotiating session…", "正在协商会话…"),
                "Authenticating":L("Authenticating…", "正在验证身份…"), "PreparingDesktop":L("Preparing desktop…", "正在准备桌面…"),
                "Reconnecting":L("Reconnecting…", "正在重新连接…")]
            status = stages[stage] ?? stage
            isConnecting = true
            if stage == "Reconnecting" { ready = false; audio.stop() }
        case "ready":
            if let resolved = event["resolved"] as? [String:Any], var command = retryCommand, var request = command["request"] as? [String:Any] {
                request["host"] = resolved["host"]; request["port"] = resolved["port"]; request["relay"] = resolved["relay"]
                command["request"] = request; command.removeValue(forKey: "credential_id"); retryCommand = command
            }
            ready = true; needsPIN = false; certificatePrompt = nil; isConnecting = false; error = nil; codec = event["codec"] as? String ?? ""
            UIApplication.shared.isIdleTimerDisabled = true
            remoteClipboard = event["clipboard"] as? Bool ?? false
            remoteAudio = event["audio"] as? Bool ?? false
            canRefresh = event["refresh"] as? Bool ?? false
            status = L("Connected", "已连接")
            if var draft = pendingInvitation, let resolved = event["resolved"] as? [String:Any], let host = resolved["host"] as? String, let port = resolved["port"] as? Int {
                pendingInvitation = nil
                draft.host = host; draft.port = String(port); draft.hostFingerprint = event["fingerprint"] as? String ?? ""
                do { try save(draft) } catch { self.error = error.localizedDescription }
            }
            if event["audio"] as? Bool == true {
                let current = generation
                Task { @MainActor [weak self] in
                    guard let self, self.ready, self.generation == current else { return }
                    do {
                        try await self.audio.start(rate:event["sample_rate"] as? Double ?? 48000,
                            channels:event["channels"] as? Int ?? 2)
                    } catch {
                        guard self.ready, self.generation == current else { return }
                        self.error = L("Audio unavailable: ", "音频不可用：") + error.localizedDescription
                    }
                }
            }
        case "closed":
            ready = false; needsPIN = false; certificatePrompt = nil; isConnecting = false; audio.stop(); UIApplication.shared.isIdleTimerDisabled = false
            status = L("Disconnected", "连接已断开"); error = event["error"] as? String
        default: break
        }
    }
}
