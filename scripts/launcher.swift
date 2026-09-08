// The bundle entry point. Installed copies exec the Rust app immediately.
// Copies opened from a read-only DMG (including App Translocation) offer a
// standard, explicit installation without shell scripts or administrator access.
import AppKit
import Darwin

let fm = FileManager.default
let source = Bundle.main.bundleURL
let chinese = Locale.preferredLanguages.first?.hasPrefix("zh") == true
func tr(_ en: String, _ zh: String) -> String { chinese ? zh : en }

func alert(_ title: String, _ message: String, _ buttons: [String]) -> NSApplication.ModalResponse {
    NSApplication.shared.setActivationPolicy(.regular)
    NSApplication.shared.activate(ignoringOtherApps: true)
    let dialog = NSAlert()
    dialog.messageText = title
    dialog.informativeText = message
    dialog.icon = NSImage(contentsOf: source.appendingPathComponent("Contents/Resources/AppIcon.icns"))
    buttons.forEach { dialog.addButton(withTitle: $0) }
    return dialog.runModal()
}

func run(_ executable: String, _ arguments: [String]) throws {
    let p = Process()
    p.executableURL = URL(fileURLWithPath: executable)
    p.arguments = arguments
    try p.run()
    p.waitUntilExit()
    guard p.terminationStatus == 0 else {
        throw NSError(domain: "RemoventInstaller", code: Int(p.terminationStatus),
                      userInfo: [NSLocalizedDescriptionKey: "\(executable) failed (\(p.terminationStatus))"])
    }
}

func serviceIsRunning(_ cli: URL) throws -> Bool {
    let task = Process()
    let output = Pipe()
    task.executableURL = cli
    task.arguments = ["daemon", "service-status"]
    task.standardOutput = output
    task.standardError = FileHandle.nullDevice
    try task.run()
    let data = output.fileHandleForReading.readDataToEndOfFile()
    task.waitUntilExit()
    guard task.terminationStatus == 0,
          let status = try JSONSerialization.jsonObject(with: data) as? [String: Any],
          let reachable = status["reachable"] as? Bool,
          let managed = status["managed"] as? Bool else {
        throw NSError(domain: "RemoventInstaller", code: 3, userInfo: [NSLocalizedDescriptionKey:
            tr("Could not check the existing background service.", "无法检查现有后台服务的状态。")])
    }
    return reachable || managed
}

let readOnly = (try? source.resourceValues(forKeys: [.volumeIsReadOnlyKey]))?.volumeIsReadOnly == true
if readOnly {
    let system = URL(fileURLWithPath: "/Applications", isDirectory: true)
    let directory = fm.isWritableFile(atPath: system.path) ? system : fm.homeDirectoryForCurrentUser.appendingPathComponent("Applications", isDirectory: true)
    let destination = directory.appendingPathComponent("Removent.app", isDirectory: true)
    let choice = alert(tr("Install Removent?", "安装 Removent？"),
        tr("Removent will be installed in \(directory.path) and opened. You can eject the disk image afterwards.",
           "Removent 将安装到 \(directory.path) 并启动，之后即可推出安装磁盘。"),
        [tr("Install and Open", "安装并打开"), tr("Cancel", "取消")])
    guard choice == .alertFirstButtonReturn else { exit(0) }
    let identifier = UUID().uuidString
    let staged = directory.appendingPathComponent(".Removent-install-\(identifier).app")
    let backup = directory.appendingPathComponent(".Removent-backup-\(identifier).app")
    var backedUp = false
    var installed = false
    do {
        // Replacing an active bundle could leave its daemon and tray on the old
        // version. Ask the user to quit instead of killing their sessions.
        let active = NSWorkspace.shared.runningApplications.contains {
            guard $0.processIdentifier != getpid(), let url = $0.bundleURL?.standardizedFileURL else { return false }
            return url == destination.standardizedFileURL || url.path.hasPrefix(destination.path + "/")
        }
        guard !active else {
            throw NSError(domain: "RemoventInstaller", code: 1, userInfo: [NSLocalizedDescriptionKey:
                tr("Quit the installed Removent app and menu bar app before replacing them. For an existing installation, use Software Update after disconnecting.",
                   "请先退出已安装的 Removent 主窗口和菜单栏应用。已有安装建议在断开连接后使用应用内软件更新。")])
        }
        try fm.createDirectory(at: directory, withIntermediateDirectories: true)
        try run("/usr/bin/ditto", [source.path, staged.path])
        try run("/usr/bin/codesign", ["--verify", "--deep", "--strict", staged.path])
        if fm.fileExists(atPath: destination.path) {
            guard (try destination.resourceValues(forKeys: [.isSymbolicLinkKey])).isSymbolicLink != true else {
                throw NSError(domain: "RemoventInstaller", code: 2, userInfo: [NSLocalizedDescriptionKey: "Installation destination is a symbolic link"])
            }
            // A launchd service can be alive even with both UIs closed. Do not
            // silently replace its executable or leave old code serving peers.
            if try serviceIsRunning(staged.appendingPathComponent("Contents/MacOS/removent-cli")) {
                throw NSError(domain: "RemoventInstaller", code: 4, userInfo: [NSLocalizedDescriptionKey:
                    tr("The background service is still running. Use Software Update, or stop it with removent-cli daemon stop before replacing the app.",
                       "后台服务仍在运行。请使用应用内软件更新，或先执行 removent-cli daemon stop 再替换应用。")])
            }
            try fm.moveItem(at: destination, to: backup)
            backedUp = true
        }
        try fm.moveItem(at: staged, to: destination)
        installed = true
        try run("/usr/bin/open", ["-n", destination.path, "--args"] + Array(CommandLine.arguments.dropFirst()))
        if backedUp { try? fm.removeItem(at: backup) }
        exit(0)
    } catch {
        try? fm.removeItem(at: staged)
        if installed { try? fm.removeItem(at: destination) }
        if backedUp {
            do { try fm.moveItem(at: backup, to: destination) }
            catch {
                _ = alert(tr("Restore the previous version", "请恢复之前的版本"), backup.path, ["OK"])
            }
        }
        _ = alert(tr("Installation could not finish", "安装未完成"), error.localizedDescription,
                  [tr("OK", "好")])
        exit(1)
    }
}

// A server-only entry point also works through `open --args --server` and
// never constructs the GPUI client or requires the menu bar app to stay alive.
if CommandLine.arguments.contains("--server") {
    do {
        let cli = source.appendingPathComponent("Contents/MacOS/removent-cli").path
        try run(cli, ["daemon", CommandLine.arguments.contains("--login") ? "login-on" : "start"])
        if CommandLine.arguments.contains("--tray") {
            try run("/usr/bin/open", ["-g", source.appendingPathComponent("Contents/Helpers/RemoventTray.app").path])
        }
        exit(0)
    } catch {
        _ = alert(tr("Could not start the background service", "无法启动后台服务"),
                  error.localizedDescription, [tr("OK", "好")])
        exit(1)
    }
}

let binary = source.appendingPathComponent("Contents/MacOS/removent").path
let arguments = [binary] + Array(CommandLine.arguments.dropFirst())
let pointers = arguments.map { strdup($0) } + [nil]
pointers.withUnsafeBufferPointer { buffer in
    _ = execv(binary, buffer.baseAddress!)
}
perror("Removent")
exit(1)
