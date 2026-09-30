import Foundation
import CloudKit

typealias SyncStoreCall = @MainActor ([String: Any]) async throws -> [String: Any]

enum ConnectionSyncError: Error, LocalizedError {
    case configuration, invalidRecord, accountChanged, storage(String)
    var errorDescription: String? {
        switch self {
        case .configuration: return "iCloud is not configured for this build."
        case .invalidRecord: return "This connection requires a newer version of Removent."
        case .accountChanged: return "The iCloud account changed."
        case .storage(let message): return message
        }
    }
}

struct CloudSyncStatus: Decodable {
    var enabled = false
    var account_available = false
    var code = "off"
    var pending = 0
    var conflicts: [CloudSyncConflict] = []
    var last_success: Double = 0
    var retry: UInt64 = 0
}
struct CloudSyncConflict: Decodable, Identifiable {
    let id: String
    let local: CloudSyncConnection?
    let remote: CloudSyncConnection?
}
struct CloudSyncConnection: Decodable {
    struct Relay: Decodable {
        let endpoint: String
        let transport: String
        let server_fingerprint: String
        let host_fingerprint: String
    }
    let name: String
    let host: String
    let port: Int
    let username: String
    let domain: String
    let `protocol`: String
    let relay: Relay?
    func summary(relayLabel: String, relayIDLabel: String, hostIDLabel: String) -> String {
        let address = port == 0 ? host : "\(host.contains(":") ? "[\(host)]" : host):\(port)"
        var parts = ["\(name.isEmpty ? host : name) · \(`protocol`.uppercased()) · \(address)"]
        if !username.isEmpty { parts.append(domain.isEmpty ? username : domain + "/" + username) }
        if let relay {
            parts.append("\(relayLabel): \(relay.endpoint) · \(relay.transport.uppercased())")
            func short(_ value: String) -> String { value.count > 24 ? String(value.prefix(12)) + "…" + String(value.suffix(8)) : value }
            if !relay.server_fingerprint.isEmpty { parts.append("\(relayIDLabel): \(short(relay.server_fingerprint))") }
            parts.append("\(hostIDLabel): \(short(relay.host_fingerprint))")
        }
        return parts.joined(separator: "\n")
    }
}

@MainActor
final class CloudSyncStore {
    let call: SyncStoreCall
    let scope: String
    let isCurrent: @MainActor () -> Bool
    let zoneID = CKRecordZone.ID(zoneName: "RemoventConnections", ownerName: CKCurrentUserDefaultName)
    init(call: @escaping SyncStoreCall, scope: String, isCurrent: @escaping @MainActor () -> Bool) {
        self.call = call; self.scope = scope; self.isCurrent = isCurrent
    }
    @discardableResult
    func command(_ op: String, _ fields: [String: Any] = [:]) async throws -> [String: Any] {
        guard isCurrent() else { throw ConnectionSyncError.accountChanged }
        var input = fields; input["op"] = op; input["scope"] = scope
        return try await call(input)
    }
    func snapshot() async throws -> [String: Any] { try await command("snapshot") }
    func pending() async throws -> [[String: Any]] { try await snapshot()["pending"] as? [[String: Any]] ?? [] }
    func checkpoint(_ value: String) async throws { try await command("checkpoint", ["value": value]) }

    func encodeRecord(_ value: [String: Any]) throws -> CKRecord {
        guard let id = value["id"] as? String, let payload = value["payload"] as? [String: Any] else { throw ConnectionSyncError.invalidRecord }
        let record: CKRecord
        if let encoded = value["system_fields"] as? String, let data = Data(base64Encoded: encoded) {
            let decoder = try NSKeyedUnarchiver(forReadingFrom: data)
            decoder.requiresSecureCoding = true; defer { decoder.finishDecoding() }
            guard let stored = CKRecord(coder: decoder), stored.recordID.recordName == id,
                  stored.recordID.zoneID == zoneID, stored.recordType == "SavedConnection" else { throw ConnectionSyncError.invalidRecord }
            record = stored
        } else { record = CKRecord(recordType: "SavedConnection", recordID: .init(recordName: id, zoneID: zoneID)) }
        record.encryptedValues["payload"] = try JSONSerialization.data(withJSONObject: payload, options: [.sortedKeys]) as NSData
        return record
    }

    func decodeRecord(_ record: CKRecord, baseRevision: Any = NSNull()) throws -> [String: Any] {
        guard record.recordID.zoneID == zoneID, record.recordType == "SavedConnection",
              let data = record.encryptedValues["payload"] as? Data, data.count <= 64 * 1024,
              let payload = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              payload["version"] as? Int == 1 else { throw ConnectionSyncError.invalidRecord }
        // Refuse unknown payload fields rather than overwriting data from a newer client.
        guard Set(payload.keys).isSubset(of: ["version", "revision", "data"]) else { throw ConnectionSyncError.invalidRecord }
        if let fields = payload["data"] as? [String: Any] {
            guard Set(fields.keys).isSubset(of: ["name", "protocol", "host", "port", "username", "domain", "relay"]) else { throw ConnectionSyncError.invalidRecord }
        }
        let coder = NSKeyedArchiver(requiringSecureCoding: true)
        record.encodeSystemFields(with: coder); coder.finishEncoding()
        return ["id": record.recordID.recordName, "payload": payload, "system_fields": coder.encodedData.base64EncodedString(), "base_revision": baseRevision]
    }
    func apply(_ records: [CKRecord], removed: [CKRecord.ID] = []) async throws {
        let values = try records.map { try decodeRecord($0) }
        if !values.isEmpty { try await command("apply", ["records": values]) }
        let ids = removed.filter { $0.zoneID == zoneID }.map(\.recordName)
        if !ids.isEmpty { try await command("removed", ["ids": ids]) }
    }
    func acknowledge(_ records: [CKRecord], sent: [String: [String: Any]]) async throws {
        let values = try records.map { record in
            try decodeRecord(record, baseRevision: sent[record.recordID.recordName]?["base_revision"] ?? NSNull())
        }
        if !values.isEmpty { try await command("acknowledge", ["records": values]) }
    }
}

@MainActor
func ensureConnectionZone(database: CKDatabase, store: CloudSyncStore) async throws -> Bool {
    do { _ = try await database.recordZone(for: store.zoneID); return false }
    catch let error as CKError where error.code == .zoneNotFound || error.code == .userDeletedZone {
        // Detect recreation before fetching with a token from the previous zone.
        try await store.command("reset", ["zone_deleted": true])
        _ = try await database.save(CKRecordZone(zoneID: store.zoneID))
        return true
    }
}

func syncErrorCode(_ error: Error) -> String {
    guard let error = error as? CKError else {
        if case ConnectionSyncError.configuration = error { return "configuration" }
        if case ConnectionSyncError.invalidRecord = error { return "upgrade_required" }
        return "error"
    }
    switch error.code {
    case .networkFailure, .networkUnavailable, .serviceUnavailable, .requestRateLimited, .zoneBusy: return "offline"
    case .notAuthenticated, .accountTemporarilyUnavailable: return "account_unavailable"
    case .quotaExceeded: return "quota"
    case .badContainer, .badDatabase, .missingEntitlement, .permissionFailure: return "configuration"
    default: return "error"
    }
}
