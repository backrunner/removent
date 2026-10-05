import Foundation

enum ConnectionProtocol: String, CaseIterable, Codable, Identifiable {
    case removent, vnc, rdp
    var id: String { rawValue }
    var title: String { self == .removent ? "Removent" : rawValue.uppercased() }
    var port: Int { switch self { case .removent: 48688; case .vnc: 5900; case .rdp: 3389 } }
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
}

struct NearbyDevice: Identifiable {
    var id: String
    var name: String
    var host: String
    var port: Int
    var `protocol`: ConnectionProtocol
    var pairingLocator: String? = nil
    var pairingExpiresAt: TimeInterval? = nil
    var pairingIdentity: String? = nil
    var endpointKey: String { "\(`protocol`.rawValue):\(host.lowercased()):\(port)" }
}

struct ConnectionDraft: Identifiable {
    var id = UUID()
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
            "audio":audio, "clipboard":clipboard]
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
