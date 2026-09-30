import AppKit
import Darwin
import Foundation

// CloudKit runs in this provisioned, user-owned bundle, never in removentd or
// the launcher that execs the Rust GUI. The CLI is only the local storage bridge.
@MainActor
final class SyncHelperDelegate: NSObject, NSApplicationDelegate {
    var service: ConnectionSyncService?
    let directory: String
    let cli: URL
    private var lifetime: Timer?
    init(directory: String, cli: URL) { self.directory = directory; self.cli = cli }
    func applicationDidFinishLaunching(_ notification: Notification) {
        let directory = directory; let cli = cli
        let originalInode = (try? FileManager.default.attributesOfItem(atPath: cli.path)[.systemFileNumber]) as? NSNumber
        let arguments = CommandLine.arguments
        guard let index = arguments.firstIndex(of: "--parent-pid"), arguments.indices.contains(index + 1),
              let parent = Int32(arguments[index + 1]), parent > 1 else { NSApplication.shared.terminate(nil); return }
        lifetime = Timer.scheduledTimer(withTimeInterval: 2, repeats: true) { _ in
            let inode = (try? FileManager.default.attributesOfItem(atPath: cli.path)[.systemFileNumber]) as? NSNumber
            if kill(parent, 0) != 0 || inode != originalInode { NSApplication.shared.terminate(nil) }
        }
        service = ConnectionSyncService { command in
            let input = try JSONSerialization.data(withJSONObject: command)
            let output = try await Task.detached(priority: .utility) {
                let process = Process(); process.executableURL = cli; process.arguments = ["cloud-sync"]
                var environment = ProcessInfo.processInfo.environment
                environment["REMOVENT_DATA_DIR"] = directory; process.environment = environment
                let stdin = Pipe(); let stdout = Pipe()
                process.standardInput = stdin; process.standardOutput = stdout; process.standardError = FileHandle.nullDevice
                try process.run()
                let timeout = DispatchWorkItem { if process.isRunning { process.terminate() } }
                DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + 30, execute: timeout)
                defer { timeout.cancel() }
                try stdin.fileHandleForWriting.write(contentsOf: input); try stdin.fileHandleForWriting.close()
                let data = try stdout.fileHandleForReading.readToEnd() ?? Data()
                process.waitUntilExit()
                guard process.terminationStatus == 0, data.count <= 8 * 1024 * 1024 else {
                    throw ConnectionSyncError.storage("Connection storage is unavailable.")
                }
                return data
            }.value
            guard let response = try JSONSerialization.jsonObject(with: output) as? [String: Any],
                  response["ok"] as? Bool == true, let value = response["value"] as? [String: Any] else {
                throw ConnectionSyncError.storage("Could not update connection storage.")
            }
            if value["enabled"] as? Bool == false { NSApplication.shared.terminate(nil) }
            return value
        }
        NSApplication.shared.registerForRemoteNotifications()
        service?.start()
    }
    func application(_ application: NSApplication, didReceiveRemoteNotification userInfo: [String: Any]) { service?.wake() }
    func applicationDidBecomeActive(_ notification: Notification) { service?.wake() }
}

guard getuid() != 0 else { exit(1) }
let arguments = CommandLine.arguments
guard let flag = arguments.firstIndex(of: "--data-dir"), arguments.indices.contains(flag + 1) else { exit(2) }
let directory = URL(fileURLWithPath: arguments[flag + 1], isDirectory: true).standardizedFileURL
try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
let lock = open(directory.appendingPathComponent(".cloud-sync-owner.lock").path, O_CREAT | O_RDWR | O_NOFOLLOW, 0o600)
guard lock >= 0, flock(lock, LOCK_EX | LOCK_NB) == 0 else { exit(0) }
let parentContents = Bundle.main.bundleURL.deletingLastPathComponent().deletingLastPathComponent()
let cli = parentContents.appendingPathComponent("MacOS/removent-cli")
guard FileManager.default.isExecutableFile(atPath: cli.path) else { exit(3) }
MainActor.assumeIsolated {
    let app = NSApplication.shared
    let delegate = SyncHelperDelegate(directory: directory.path, cli: cli)
    app.setActivationPolicy(.prohibited); app.delegate = delegate; app.run()
}
