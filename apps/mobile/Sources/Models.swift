import Foundation
import Darwin

// Normalize only local strings supplied by bookmarks or Bonjour, never resolve
// arbitrary hosts or open discovery connections.
private func endpointHostKey(_ value: String) -> String {
    var host = value.trimmingCharacters(in: .whitespacesAndNewlines)
    if host.hasPrefix("[") && host.hasSuffix("]") { host = String(host.dropFirst().dropLast()) }
    let parts = host.split(separator: "%", maxSplits: 1, omittingEmptySubsequences: false)
    let address = String(parts[0])
    var ipv6 = in6_addr()
    if inet_pton(AF_INET6, address, &ipv6) == 1 {
        var buffer = [CChar](repeating: 0, count: Int(INET6_ADDRSTRLEN))
        _ = inet_ntop(AF_INET6, &ipv6, &buffer, socklen_t(buffer.count))
        host = String(cString: buffer)
        if parts.count == 2 {
            let scope = String(parts[1])
            let index = UInt32(scope) ?? if_nametoindex(scope)
            host += "%" + (index == 0 ? scope : String(index))
        }
        return host
    }
    return host.lowercased().trimmingCharacters(in: CharacterSet(charactersIn: "."))
}

enum ConnectionProtocol: String, CaseIterable, Codable, Identifiable {
    case removent, vnc, rdp
    var id: String { rawValue }
    var title: String { self == .removent ? "Removent" : rawValue.uppercased() }
    var port: Int { switch self { case .removent: 48688; case .vnc: 5900; case .rdp: 3389 } }
}

enum VideoQuality: String, CaseIterable, Codable, Identifiable {
    case low, standard, high, ultra
    var id: String { rawValue }
    var title: String { switch self {
        case .low: L("Low", "低")
        case .standard: L("Standard", "标准")
        case .high: L("High (2K)", "高（2K）")
        case .ultra: L("Ultra (4K)", "超高（4K）")
    } }
    var range: String { switch self {
        case .low: "480–720p"
        case .standard: "540–1080p"
        case .high: "540–1440p"
        case .ultra: "540–2160p"
    } }
}

struct RelayRoute: Codable, Equatable {
    var endpoint: String
    var transport: String
    var server_name: String? = nil
    var accept_invalid_certificate: Bool? = nil
    var server_fingerprint: String
    var host_fingerprint: String
}

struct Bookmark: Codable, Identifiable {
    var video_quality: VideoQuality? = nil
    var id: String
    var name: String
    var `protocol`: ConnectionProtocol
    var host: String
    var port: Int
    var username: String
    var domain: String
    var accept_invalid_certificate: Bool
    var relay: RelayRoute?
    var credentials_review_required: Bool
    var password_hint: Bool
    var title: String { name.isEmpty ? host : name }

    func matches(_ device: NearbyDevice) -> Bool {
        relay == nil && `protocol` == device.protocol && port == device.port
            && ([device.host] + device.advertisedHosts).contains { endpointHostKey(host) == endpointHostKey($0) }
    }
}

struct NearbyDevice: Identifiable, Equatable {
    var id: String
    var name: String
    var host: String
    var port: Int
    var `protocol`: ConnectionProtocol
    var advertisedHosts: [String] = []
    var pairingLocator: String? = nil
    var pairingExpiresAt: TimeInterval? = nil
    var pairingIdentity: String? = nil
    var endpointKey: String { "\(`protocol`.rawValue):\(endpointHostKey(host)):\(port)" }

    static func visible(_ devices: [NearbyDevice], excluding bookmarks: [Bookmark]) -> [NearbyDevice] {
        var seen = Set<String>()
        return devices.filter { device in
            !bookmarks.contains { $0.matches(device) } && seen.insert(device.endpointKey).inserted
        }
    }
}

struct ConnectionDraft: Identifiable {
    var id = UUID()
    var videoQuality = VideoQuality.standard
    var bookmarkID: String?
    var name = ""
    var `protocol` = ConnectionProtocol.removent
    var host = ""
    var port = "48688"
    var username = ""
    var password = ""
    var domain = ""
    var allowUntrustedCertificate = false
    var useRelay = false
    var relayEndpoint = ""
    var relayTransport = "websocket"
    var relayServerName = ""
    var verifyRelayCertificate = true
    var relayFingerprint = ""
    var hostFingerprint = ""
    var replaceSavedPassword = false
    var rememberAfterPairing = false
    private var original: Bookmark?

    init() {}
    init(_ device: NearbyDevice) {
        name = device.name; `protocol` = device.protocol; host = device.host; port = String(device.port)
    }
    init(_ bookmark: Bookmark) {
        original = bookmark
        videoQuality = bookmark.video_quality ?? .standard
        bookmarkID = bookmark.id; name = bookmark.name; `protocol` = bookmark.protocol
        host = bookmark.host; port = String(bookmark.port); username = bookmark.username
        domain = bookmark.domain; allowUntrustedCertificate = bookmark.accept_invalid_certificate
        if let relay = bookmark.relay {
            useRelay = true; relayEndpoint = relay.endpoint; relayTransport = relay.transport
            relayFingerprint = relay.server_fingerprint; hostFingerprint = relay.host_fingerprint
            relayServerName = relay.server_name ?? ""
            verifyRelayCertificate = !(relay.accept_invalid_certificate ?? false)
        }
    }
    var isPairingCode: Bool {
        let code = host.filter { !$0.isWhitespace && $0 != "-" }
        return `protocol` == .removent && code.utf8.count == 12 && code.utf8.allSatisfy { (48...57).contains($0) }
    }
    var title: String { name.isEmpty ? host : name }
    var hasSavedPassword: Bool { original?.password_hint == true }
    var credentialID: String? {
        hasSavedPassword && !replaceSavedPassword && password.isEmpty ? bookmarkID : nil
    }
    func request(audio: Bool, clipboard: Bool) throws -> [String: Any] {
        let host = host.trimmingCharacters(in: .whitespacesAndNewlines)
        let port = port.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !host.isEmpty else {
            throw MobileError.message(L("Enter a hostname or IP address.", "请输入主机名或 IP 地址。"))
        }
        guard !useRelay || `protocol` == .removent else {
            throw MobileError.message(L("Relay requires Removent.", "中继连接需要使用 Removent 协议。"))
        }
        guard useRelay || isPairingCode || (Int(port).map { (1...65535).contains($0) } ?? false) else {
            throw MobileError.message(L("Port must be between 1 and 65535", "端口必须为 1–65535"))
        }
        if `protocol` == .rdp && username.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            throw MobileError.message(L("Enter an RDP username.", "请输入 RDP 用户名。"))
        }
        var result: [String: Any] = ["protocol":`protocol`.rawValue, "host":host,
            "port":useRelay ? 0 : (isPairingCode ? 48688 : Int(port)!), "username":username, "password":password,
            "domain":domain, "accept_invalid_certificate":allowUntrustedCertificate,
            "audio":audio, "clipboard":clipboard, "video_quality":videoQuality.rawValue]
        if useRelay {
            result["relay"] = ["endpoint":relayEndpoint, "transport":relayTransport,
                "server_name":relayServerName, "accept_invalid_certificate":!verifyRelayCertificate,
                "server_fingerprint":relayTransport == "quic" ? relayFingerprint : "",
                "host_fingerprint":isPairingCode ? "" : hostFingerprint]
        }
        return result
    }
}

enum MobileError: LocalizedError {
    case message(String)
    var errorDescription: String? { if case .message(let message) = self { return message }; return nil }
}

func L(_ en: String, _ zh: String) -> String {
    Locale.preferredLanguages.first?.hasPrefix("zh") == true ? zh : en
}
