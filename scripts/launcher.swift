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
            $0.processIdentifier != getpid() && $0.bundleURL?.standardizedFileURL == destination.standardizedFileURL
        }
        guard !active else {
            throw NSError(domain: "RemoventInstaller", code: 1, userInfo: [NSLocalizedDescriptionKey:
                tr("Quit the installed Removent app before replacing it. For a running session, use Software Update after disconnecting.",
                   "请先退出已安装的 Removent 再安装。若会话正在进行，请在断开后使用应用内软件更新。")])
        }
        try fm.createDirectory(at: directory, withIntermediateDirectories: true)
        try run("/usr/bin/ditto", [source.path, staged.path])
        try run("/usr/bin/codesign", ["--verify", "--deep", "--strict", staged.path])
        if fm.fileExists(atPath: destination.path) {
            guard (try destination.resourceValues(forKeys: [.isSymbolicLinkKey])).isSymbolicLink != true else {
                throw NSError(domain: "RemoventInstaller", code: 2, userInfo: [NSLocalizedDescriptionKey: "Installation destination is a symbolic link"])
            }
            try fm.moveItem(at: destination, to: backup)
            backedUp = true
        }
        try fm.moveItem(at: staged, to: destination)
        installed = true
        try run("/usr/bin/open", ["-n", destination.path])
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

let binary = source.appendingPathComponent("Contents/MacOS/removent").path
let arguments = [binary] + Array(CommandLine.arguments.dropFirst())
let pointers = arguments.map { strdup($0) } + [nil]
pointers.withUnsafeBufferPointer { buffer in
    _ = execv(binary, buffer.baseAddress!)
}
perror("Removent")
exit(1)
