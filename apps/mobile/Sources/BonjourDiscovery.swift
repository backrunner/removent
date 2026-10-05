import Foundation
import Combine

// Native Bonjour uses iOS local-network permission without a raw multicast entitlement.
@MainActor
final class BonjourDiscovery: NSObject, ObservableObject, @preconcurrency NetServiceBrowserDelegate, @preconcurrency NetServiceDelegate {
    @Published var devices: [NearbyDevice] = []
    @Published var error: String?
    private var browsers: [NetServiceBrowser] = []
    private var services: [String:NetService] = [:]

    func start(removent: Bool, vnc: Bool, rdp: Bool) {
        stop(); error = nil
        var types: [String] = []
        if removent { types += ["_removent._udp."] }
        if vnc { types += ["_rfb._tcp."] }
        if rdp { types += ["_rdp._tcp.", "_ms-wbt-server._tcp."] }
        for type in types {
            let browser = NetServiceBrowser(); browser.delegate = self
            browsers.append(browser); browser.searchForServices(ofType:type, inDomain:"local.")
        }
    }
    func stop() {
        for browser in browsers { browser.delegate = nil; browser.stop() }
        for service in services.values { service.delegate = nil; service.stop() }
        browsers = []; services = [:]; devices = []
    }
    private func key(_ service: NetService) -> String { "\(service.type)\(service.name).\(service.domain)" }
    func netServiceBrowser(_ browser: NetServiceBrowser, didFind service: NetService, moreComing: Bool) {
        services[key(service)] = service; service.delegate = self; service.resolve(withTimeout:8)
    }
    func netServiceBrowser(_ browser: NetServiceBrowser, didRemove service: NetService, moreComing: Bool) {
        let id = key(service); services.removeValue(forKey:id)?.stop(); devices.removeAll { $0.id == id }
    }
    func netServiceBrowser(_ browser: NetServiceBrowser, didNotSearch errorDict: [String:NSNumber]) {
        error = L("Allow Local Network access in Settings to discover computers.", "请在系统设置中允许访问本地网络，以发现附近电脑。")
    }
    func netServiceDidResolveAddress(_ service: NetService) {
        guard services[key(service)] === service, service.port > 0 else { return }
        // Prefer a numeric address so saved discovery also works when multicast
        // DNS is later unavailable. getnameinfo retains IPv6 interface scope.
        let host = service.addresses?.compactMap { data -> String? in
            data.withUnsafeBytes { bytes in
                guard let base = bytes.baseAddress, data.count >= MemoryLayout<sockaddr>.size else { return nil }
                let address = base.assumingMemoryBound(to:sockaddr.self)
                var result = [CChar](repeating:0, count:Int(NI_MAXHOST))
                guard getnameinfo(address, socklen_t(data.count), &result, socklen_t(result.count),
                    nil, 0, NI_NUMERICHOST) == 0 else { return nil }
                return String(cString:result)
            }
        }.sorted { !$0.contains(":") && $1.contains(":") }.first ?? service.hostName
        guard let host else { return }
        let proto: ConnectionProtocol = service.type == "_removent._udp." ? .removent : service.type == "_rfb._tcp." ? .vnc : .rdp
        let id = key(service)
        devices.removeAll { $0.id == id }
        let txt = service.txtRecordData().map(NetService.dictionary(fromTXTRecord:)) ?? [:]
        let locator = txt["pair"].flatMap { String(data:$0, encoding:.utf8) }
        let expires = txt["pair-exp"].flatMap { String(data:$0, encoding:.utf8) }.flatMap(TimeInterval.init)
        devices.append(NearbyDevice(id:id, name:service.name, host:host, port:service.port, protocol:proto, pairingLocator:locator, pairingExpiresAt:expires, pairingIdentity:txt["fp"].flatMap { String(data:$0, encoding:.utf8) }))
        devices.sort { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
    }

    static func resolvePairing(_ locator: String) async throws -> NearbyDevice {
        let discovery = BonjourDiscovery()
        discovery.start(removent:true, vnc:false, rdp:false)
        defer { discovery.stop() }
        let deadline = Date().addingTimeInterval(10)
        var settled = false
        while Date() < deadline {
            if let error = discovery.error { throw MobileError.message(error) }
            let announced = discovery.devices.filter { $0.protocol == .removent && $0.pairingLocator == locator && ($0.pairingExpiresAt ?? 0) > Date().timeIntervalSince1970 }
            // A computer may advertise the same identity on several interfaces
            // or with a Bonjour conflict suffix. Count computers, not records.
            let matches = Dictionary(grouping:announced, by: { $0.pairingIdentity ?? $0.endpointKey }).values.compactMap { $0.first }
            if matches.count > 1 { throw MobileError.message(L("Ambiguous code; generate a new connection code.", "连接码重复，请在电脑上重新生成。")) }
            if let device = matches.first {
                if settled { return device }
                settled = true
                try await Task.sleep(for:.milliseconds(300))
            } else { try await Task.sleep(for:.milliseconds(100)) }
        }
        throw MobileError.message(L("Connection code not found on this network or expired.", "当前网络中未找到此连接码，或连接码已过期。"))
    }
}
