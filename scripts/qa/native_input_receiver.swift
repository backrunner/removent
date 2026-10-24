// AppKit receiver for the opt-in native QUIC input acceptance test.
import AppKit

let output = URL(fileURLWithPath: CommandLine.arguments[1])
let ready = URL(fileURLWithPath: CommandLine.arguments[2])
FileManager.default.createFile(atPath: output.path, contents: nil)
let log = try FileHandle(forWritingTo: output)
let previous = NSWorkspace.shared.frontmostApplication
let app = NSApplication.shared
app.setActivationPolicy(.regular)

final class InputView: NSView {
    override var acceptsFirstResponder: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override func keyDown(with event: NSEvent) {}
    override func keyUp(with event: NSEvent) {}
}
let screen = NSScreen.screens[0]
let frame = NSRect(x: screen.frame.minX + 100, y: screen.frame.minY + 200, width: 400, height: 300)
let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
window.title = "Removent input acceptance"
window.contentView = InputView(frame: NSRect(origin: .zero, size: frame.size))
window.makeKeyAndOrderFront(nil)
window.makeFirstResponder(window.contentView)
app.activate(ignoringOtherApps: true)
let monitor = NSEvent.addLocalMonitorForEvents(matching: [
    .leftMouseDown, .leftMouseUp, .rightMouseDown, .rightMouseUp,
    .otherMouseDown, .otherMouseUp, .leftMouseDragged, .rightMouseDragged,
    .otherMouseDragged, .keyDown, .keyUp, .flagsChanged, .scrollWheel
]) { event in
    var data: [String: Any] = ["type": event.type.rawValue, "flags": event.modifierFlags.rawValue]
    switch event.type {
    case .leftMouseDown, .leftMouseUp, .rightMouseDown, .rightMouseUp, .otherMouseDown, .otherMouseUp:
        data["clicks"] = event.clickCount
    case .keyDown, .keyUp:
        data["repeat"] = event.isARepeat
        data["key"] = event.keyCode
        data["text"] = event.characters ?? ""
    case .scrollWheel:
        data["phase"] = event.phase.rawValue
        data["dx"] = event.scrollingDeltaX
        data["dy"] = event.scrollingDeltaY
    default: break
    }
    if let encoded = try? JSONSerialization.data(withJSONObject: data) {
        log.write(encoded); log.write(Data([10]))
    }
    return event
}
DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
    let center = window.convertToScreen(NSRect(x: 200, y: 150, width: 0, height: 0)).origin
    let data = try! JSONSerialization.data(withJSONObject: ["x": center.x, "y": screen.frame.maxY - center.y])
    try! data.write(to: ready)
}
// A bounded lifetime restores focus even after a failed test.
Timer.scheduledTimer(withTimeInterval: 8, repeats: false) { _ in
    window.orderOut(nil)
    previous?.activate()
    app.terminate(nil)
}
app.run()
