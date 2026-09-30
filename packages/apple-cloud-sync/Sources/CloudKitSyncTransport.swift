import CloudKit
import Foundation

@MainActor
final class CloudKitSyncTransport: CKSyncEngineDelegate {
    let store: CloudSyncStore
    private var engine: CKSyncEngine!
    private var failure: Error?
    private var cancelled = false
    private var sent: [String: [String: Any]] = [:]

    init(database: CKDatabase, store: CloudSyncStore, serialized: String?) throws {
        self.store = store
        let state = try serialized.map { encoded -> CKSyncEngine.State.Serialization in
            guard let data = Data(base64Encoded: encoded) else { throw ConnectionSyncError.invalidRecord }
            return try JSONDecoder().decode(CKSyncEngine.State.Serialization.self, from: data)
        }
        var configuration = CKSyncEngine.Configuration(database: database, stateSerialization: state, delegate: self)
        // App lifecycle/push wakes drive a serialized fetch-then-send cycle. The
        // engine must not upload before enrollment or after an account switch.
        configuration.automaticallySync = false
        configuration.subscriptionID = "removent-engine-v1"
        engine = CKSyncEngine(configuration)
    }
    func cancel() async { cancelled = true; await engine.cancelOperations() }
    func synchronize() async throws {
        guard !cancelled, store.isCurrent() else { throw ConnectionSyncError.accountChanged }
        failure = nil
        if try await ensureConnectionZone(database: engine.database, store: store) {
            var configuration = CKSyncEngine.Configuration(database: engine.database, stateSerialization: nil, delegate: self)
            configuration.automaticallySync = false
            configuration.subscriptionID = "removent-engine-v1"
            engine = CKSyncEngine(configuration)
        }
        try await engine.fetchChanges(.init(scope: .zoneIDs([store.zoneID])))
        if let failure { throw failure }
        for _ in 0..<20 {
            let pending = try await store.pending()
            if pending.isEmpty { return }
            engine.state.remove(pendingRecordZoneChanges: engine.state.pendingRecordZoneChanges)
            engine.state.add(pendingRecordZoneChanges: pending.compactMap { value in
                (value["id"] as? String).map { .saveRecord(CKRecord.ID(recordName: $0, zoneID: store.zoneID)) }
            })
            try await engine.sendChanges(.init(scope: .zoneIDs([store.zoneID])))
            if let failure { throw failure }
        }
    }

    nonisolated func handleEvent(_ event: CKSyncEngine.Event, syncEngine: CKSyncEngine) async {
        await handle(event)
    }
    private func handle(_ event: CKSyncEngine.Event) async {
        guard !cancelled, store.isCurrent() else { return }
        // Never persist a token after an uncommitted page or unsupported payload.
        guard failure == nil else { return }
        do {
            switch event {
            case .stateUpdate(let event):
                try await store.checkpoint(JSONEncoder().encode(event.stateSerialization).base64EncodedString())
            case .accountChange(let event):
                if case .signIn = event.changeType { break }
                failure = ConnectionSyncError.accountChanged
            case .fetchedRecordZoneChanges(let event):
                try await store.apply(event.modifications.filter { $0.record.recordID.zoneID == store.zoneID }.map(\.record),
                    removed: event.deletions.map(\.recordID))
            case .fetchedDatabaseChanges(let event):
                if event.deletions.contains(where: { $0.zoneID == store.zoneID }) {
                    try await store.command("reset", ["zone_deleted": true]); failure = CKError(.zoneNotFound)
                }
            case .sentRecordZoneChanges(let event):
                try await store.acknowledge(event.savedRecords, sent: sent)
                for item in event.failedRecordSaves {
                    if item.error.code == .serverRecordChanged, let remote = item.error.serverRecord {
                        try await store.apply([remote])
                    } else {
                        if item.error.code == .zoneNotFound || item.error.code == .userDeletedZone || item.error.code == .unknownItem {
                            try await store.command("reset", ["zone_deleted": true])
                        }
                        failure = item.error
                    }
                }
            case .didFetchRecordZoneChanges(let event):
                if let error = event.error {
                    if error.code == .changeTokenExpired { try await store.command("reset", ["zone_deleted": false]) }
                    if error.code == .zoneNotFound || error.code == .userDeletedZone { try await store.command("reset", ["zone_deleted": true]) }
                    failure = error
                }
            default: break
            }
        } catch { failure = error }
    }

    nonisolated func nextRecordZoneChangeBatch(_ context: CKSyncEngine.SendChangesContext, syncEngine: CKSyncEngine) async -> CKSyncEngine.RecordZoneChangeBatch? {
        await batch(context)
    }
    private func batch(_ context: CKSyncEngine.SendChangesContext) async -> CKSyncEngine.RecordZoneChangeBatch? {
        guard !cancelled, failure == nil, store.isCurrent() else { return nil }
        do {
            let pending = try await store.pending().filter { value in
                guard let id = value["id"] as? String else { return false }
                return context.options.scope.contains(CKRecord.ID(recordName: id, zoneID: store.zoneID))
            }
            // Respect the engine's pending set, including the current send's scope.
            let requested = Set(engine.state.pendingRecordZoneChanges.compactMap { change -> String? in
                if case .saveRecord(let id) = change { return id.recordName }; return nil
            })
            let selected = Array(pending.filter { requested.contains($0["id"] as? String ?? "") }.prefix(200))
            guard !selected.isEmpty else { return nil }
            for value in selected { sent[value["id"] as! String] = value }
            return try .init(recordsToSave: selected.map(store.encodeRecord), recordIDsToDelete: [], atomicByZone: false)
        } catch { failure = error; return nil }
    }
}
