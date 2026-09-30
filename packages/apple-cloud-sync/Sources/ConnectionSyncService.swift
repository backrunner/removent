import CloudKit
import Combine
import CryptoKit
import Foundation
#if os(macOS)
import Security
#endif

@MainActor
final class ConnectionSyncService: ObservableObject {
    @Published private(set) var status = CloudSyncStatus()
    @Published private(set) var localError: String?
    var didChange: (() -> Void)?
    private let call: SyncStoreCall
    private var container: CKContainer?
    private var transport: CloudKitSyncTransport?
    private var scope: String?
    private var epoch: UInt64 = 0
    private var busy = false
    private var timer: Timer?
    private var accountObserver: NSObjectProtocol?
    private var retryAfter = Date.distantPast
    private var lastFetch = Date.distantPast
    private var lastRetry: UInt64 = 0
    private var failures = 0
    private var controlTask: Task<Void, Never>?
    private var controlIntent: UInt64 = 0
    static let containerID = "iCloud.com.alkinum.removent"

    init(call: @escaping SyncStoreCall) { self.call = call }
    deinit { timer?.invalidate(); if let accountObserver { NotificationCenter.default.removeObserver(accountObserver) } }

    func start() {
        guard timer == nil else { return }
        accountObserver = NotificationCenter.default.addObserver(forName: .CKAccountChanged, object: nil, queue: .main) { [weak self] _ in
            Task { @MainActor in await self?.accountChanged() }
        }
        timer = Timer.scheduledTimer(withTimeInterval: 10, repeats: true) { [weak self] _ in
            Task { @MainActor in await self?.refresh() }
        }
        Task { await refresh(force: true) }
    }
    func wake() { Task { await refresh(force: true) } }
    func setEnabled(_ enabled: Bool) {
        controlIntent &+= 1
        let intent = controlIntent
        let previous = controlTask
        // A late cancellation must not overwrite the user's more recent toggle.
        controlTask = Task {
            await previous?.value
            guard intent == controlIntent else { return }
            do {
                epoch &+= 1; await transport?.cancel(); transport = nil; scope = nil
                guard intent == controlIntent else { return }
                try await update(["op": "enable", "enabled": enabled])
                retryAfter = .distantPast
                await refresh(force: true)
            } catch { localError = error.localizedDescription }
        }
    }
    func retry() {
        Task {
            do { try await update(["op": "retry"]); retryAfter = .distantPast; await refresh(force: true) }
            catch { localError = error.localizedDescription }
        }
    }
    func resolve(_ id: String, keepLocal: Bool) {
        Task {
            do {
                try await update(["op": "resolve", "id": id, "keep_local": keepLocal])
                didChange?(); await refresh(force: true)
            } catch { localError = error.localizedDescription }
        }
    }
    private func update(_ command: [String: Any]) async throws {
        let result = try await call(command)
        status = try JSONDecoder().decode(CloudSyncStatus.self, from: JSONSerialization.data(withJSONObject: result))
    }
    private func accountChanged() async {
        epoch &+= 1; scope = nil
        let old = transport; transport = nil
        await old?.cancel()
        // Preserve the old profile separately before binding any new account.
        do { try await update(["op": "suspend"]); didChange?() }
        catch { localError = error.localizedDescription }
        retryAfter = .distantPast
        await refresh(force: true)
    }
    private func configuration() throws -> String {
        #if DEBUG
        if ProcessInfo.processInfo.arguments.contains("--ui-testing") { throw ConnectionSyncError.configuration }
        #endif
        #if os(macOS)
        guard let task = SecTaskCreateFromSelf(nil),
              let containers = SecTaskCopyValueForEntitlement(task, "com.apple.developer.icloud-container-identifiers" as CFString, nil) as? [String],
              containers.contains(Self.containerID),
              let environment = SecTaskCopyValueForEntitlement(task, "com.apple.developer.icloud-container-environment" as CFString, nil) as? String,
              ["Development", "Production"].contains(environment) else { throw ConnectionSyncError.configuration }
        return environment
        #else
        guard let environment = Bundle.main.object(forInfoDictionaryKey: "RemoventCloudKitEnvironment") as? String,
              ["Development", "Production"].contains(environment) else { throw ConnectionSyncError.configuration }
        return environment
        #endif
    }

    func refresh(force: Bool = false) async {
        guard !busy else { return }
        busy = true; defer { busy = false }
        do {
            try await update(["op": "status"])
            guard status.enabled else {
                await transport?.cancel(); transport = nil; scope = nil; return
            }
            let retryRequested = status.retry != lastRetry
            lastRetry = status.retry
            if retryRequested { retryAfter = .distantPast }
            guard Date() >= retryAfter else { return }
            guard force || retryRequested || status.pending > 0 || Date().timeIntervalSince(lastFetch) > 300 else { return }
            let environment = try configuration()
            if container == nil { container = CKContainer(identifier: Self.containerID) }
            guard let container else { throw ConnectionSyncError.configuration }
            let started = epoch
            guard try await container.accountStatus() == .available else {
                epoch &+= 1; await transport?.cancel(); transport = nil; scope = nil
                try await update(["op": "suspend"]); didChange?(); return
            }
            let user = try await container.userRecordID()
            guard started == epoch else { throw ConnectionSyncError.accountChanged }
            let account = SHA256.hash(data: Data(user.recordName.utf8)).map { String(format: "%02x", $0) }.joined()
            let current = Self.containerID + "/" + environment + "/" + account
            if scope != current {
                await transport?.cancel(); transport = nil
                try await update(["op": "bind", "scope": current])
                scope = current; didChange?()
            }
            guard started == epoch else { throw ConnectionSyncError.accountChanged }
            let store = CloudSyncStore(call: call, scope: current, isCurrent: { [weak self] in
                self?.epoch == started && self?.scope == current && self?.status.enabled == true
            })
            if transport == nil {
                let state = try await store.snapshot()["engine_state"] as? String
                transport = try CloudKitSyncTransport(database: container.privateCloudDatabase, store: store, serialized: state)
            }
            try await update(["op": "report", "scope": current, "code": "syncing"])
            try await transport?.synchronize()
            guard started == epoch else { throw ConnectionSyncError.accountChanged }
            try await update(["op": "report", "scope": current, "code": "ready"])
            lastFetch = Date(); failures = 0; localError = nil; didChange?()
        } catch {
            await transport?.cancel(); transport = nil
            failures = min(failures + 1, 8)
            let suggested = (error as? CKError)?.retryAfterSeconds ?? 0
            retryAfter = Date().addingTimeInterval(max(suggested, min(300, pow(2, Double(failures)) * 5)))
            var report: [String: Any] = ["op": "report", "code": syncErrorCode(error)]
            if let scope { report["scope"] = scope }
            // Local storage failures stay visible; never claim a successful sync.
            do { try await update(report) } catch { localError = error.localizedDescription }
        }
    }
}
