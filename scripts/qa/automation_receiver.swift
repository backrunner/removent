// Temporary AppKit fixture. No access to user files or network.
import AppKit
import Carbon
if let session = CGSessionCopyCurrentDictionary() as? [String: Any],
   (session["CGSSessionScreenIsLocked"] as? NSNumber)?.boolValue == true {
    FileHandle.standardError.write(Data("Native acceptance requires an unlocked desktop.\n".utf8))
    exit(78)
}
let directory = URL(fileURLWithPath: CommandLine.arguments[1])
let logURL = directory.appendingPathComponent("events.jsonl")
FileManager.default.createFile(atPath: logURL.path, contents: nil)
let log = try FileHandle(forWritingTo: logURL)
let app = NSApplication.shared
let previous = NSWorkspace.shared.frontmostApplication
let previousPointer = CGEvent(source: nil)!.location
app.setActivationPolicy(.regular)
final class Panel: NSView {
    override var acceptsFirstResponder: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override func mouseDown(with event: NSEvent) { window?.makeFirstResponder(self) }
    override func rightMouseDown(with event: NSEvent) {}
    override func keyDown(with event: NSEvent) {}
    override func keyUp(with event: NSEvent) {}
}
final class Action: NSObject {
    let count: NSTextField
    var value = 0
    init(_ count: NSTextField) { self.count = count }
    @objc func apply(_ sender: Any?) { value += 1; count.stringValue = String(value) }
}
// Build the fixture on AppKit's event loop.
DispatchQueue.main.async {
    let screen = NSScreen.screens[0]
    let window = NSWindow(contentRect: NSRect(x: screen.frame.minX + 100, y: screen.frame.minY + 200, width: 480, height: 320), styleMask: [.titled], backing: .buffered, defer: false)
    window.title = "Removent automation acceptance"
    window.setAccessibilityIdentifier("automation-window")
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 480, height: 320))
    let field = NSTextField(frame: NSRect(x: 24, y: 250, width: 430, height: 28))
    field.stringValue = "fixture"
    field.setAccessibilityIdentifier("automation-text")
    let count = NSTextField(labelWithString: "0")
    count.frame = NSRect(x: 140, y: 204, width: 80, height: 24)
    count.setAccessibilityIdentifier("automation-count")
    let action = Action(count)
    let button = NSButton(title: "Apply", target: action, action: #selector(Action.apply(_:)))
    button.frame = NSRect(x: 24, y: 200, width: 100, height: 32)
    button.setAccessibilityIdentifier("automation-button")
    let panel = Panel(frame: NSRect(x: 24, y: 24, width: 430, height: 150))
    panel.wantsLayer = true
    panel.layer?.backgroundColor = NSColor.controlBackgroundColor.cgColor
    for view in [field, count, button, panel] { content.addSubview(view) }
    window.contentView = content
    window.makeKeyAndOrderFront(nil)
    window.makeFirstResponder(field)
    field.selectText(nil)
    app.activate(ignoringOtherApps: true)
    let monitor = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown,.leftMouseUp,.rightMouseDown,.rightMouseUp,.leftMouseDragged,.keyDown,.keyUp,.flagsChanged,.scrollWheel]) { event in
        var value: [String: Any] = ["type": event.type.rawValue, "flags": event.modifierFlags.rawValue]
        switch event.type {
        case .leftMouseDown,.leftMouseUp,.rightMouseDown,.rightMouseUp: value["clicks"] = event.clickCount
        case .keyDown,.keyUp: value["key"] = event.keyCode; value["text"] = event.characters ?? ""
        case .scrollWheel: value["phase"] = event.phase.rawValue; value["dy"] = event.scrollingDeltaY
        default: break
        }
        if let data = try? JSONSerialization.data(withJSONObject: value) { log.write(data); log.write(Data([10])) }
        return event
    }
    DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
        let point = window.convertToScreen(NSRect(x: 150, y: 90, width: 0, height: 0)).origin
        let ready: [String: Any] = ["pid": ProcessInfo.processInfo.processIdentifier, "window_number": window.windowNumber, "visible":window.isVisible, "x": point.x, "y": screen.frame.maxY - point.y, "display_id": screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")]!]
        try! JSONSerialization.data(withJSONObject: ready).write(to: directory.appendingPathComponent("ready.json"))
    }
    let deadline = Date().addingTimeInterval(35)
    var externalKeyHeld = false
    var secureInputOwned = false
    func releaseExternalKey() {
        if externalKeyHeld {
            CGEvent(keyboardEventSource: CGEventSource(stateID: .privateState), virtualKey: 0, keyDown: false)?.post(tap: .cghidEventTap)
            externalKeyHeld = false
        }
    }
    func disableOwnedSecureInput() {
        if secureInputOwned { DisableSecureEventInput(); secureInputOwned = false }
    }
    Timer.scheduledTimer(withTimeInterval: 0.03, repeats: true) { _ in
        withExtendedLifetime((action, monitor)) {}
        if !app.isActive { releaseExternalKey(); disableOwnedSecureInput() }
        let commandURL = directory.appendingPathComponent("command.json")
        if let data = try? Data(contentsOf: commandURL),
           let command = (try? JSONSerialization.jsonObject(with: data)) as? [String: String],
           let id = command["id"], let operation = command["operation"] {
            try? FileManager.default.removeItem(at: commandURL)
            var result = "ok"
            switch operation {
            case "key_down" where app.isActive && !externalKeyHeld:
                if let event = CGEvent(keyboardEventSource: CGEventSource(stateID: .privateState), virtualKey: 0, keyDown: true) {
                    // Deliberately unmarked, from a different process: exercise the real OS tap.
                    externalKeyHeld = true
                    event.post(tap: .cghidEventTap)
                } else { result = "event_unavailable" }
            case "key_up": releaseExternalKey()
            case "secure_on" where app.isActive && !secureInputOwned:
                secureInputOwned = EnableSecureEventInput() == noErr
                if !secureInputOwned { result = "secure_input_failed" }
            case "secure_off": disableOwnedSecureInput()
            default: result = "inactive_or_invalid_command"
            }
            if let reply = try? JSONSerialization.data(withJSONObject: ["id": id, "result": result]) {
                try? reply.write(to: directory.appendingPathComponent("reply.json"), options: .atomic)
            }
        }
        if let data = try? JSONSerialization.data(withJSONObject: ["text": field.stringValue, "count": action.value, "active": app.isActive]) {
            try? data.write(to: directory.appendingPathComponent("ui.json"), options: .atomic)
        }
        if FileManager.default.fileExists(atPath: directory.appendingPathComponent("quit").path) || Date() >= deadline {
            releaseExternalKey()
            disableOwnedSecureInput()
            let restore = NSWorkspace.shared.frontmostApplication?.processIdentifier == ProcessInfo.processInfo.processIdentifier
            window.orderOut(nil)
            if restore { previous?.activate(); CGWarpMouseCursorPosition(previousPointer) }
            app.terminate(nil)
        }
    }
}
app.run()
