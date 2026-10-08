import AppKit
import SwiftUI

private final class TrayHostingController: NSHostingController<TrayPanel> {
    var onLayout: (() -> Void)?

    override func viewDidLayout() {
        super.viewDidLayout()
        onLayout?()
    }
}

extension StatusBarController {
    func configurePopover() {
        let hosting = TrayHostingController(rootView: TrayPanel(controller: self))
        // Do not let SwiftUI impose its unscrolled minimum height on the window.
        hosting.sizingOptions = [.preferredContentSize]
        hosting.onLayout = { [weak self] in
            DispatchQueue.main.async { self?.confinePopover() }
        }
        popover.contentViewController = hosting
        popover.delegate = self
        let center = NotificationCenter.default
        popoverObservers.append(center.addObserver(
            forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main
        ) { [weak self] _ in
            self?.updatePopoverLimits()
            self?.confinePopover()
        })
        popoverObservers.append(center.addObserver(
            forName: NSWindow.didMoveNotification, object: nil, queue: .main
        ) { [weak self] notification in
            guard let self, let window = notification.object as? NSWindow,
                  window === self.popover.contentViewController?.view.window else { return }
            self.confinePopover()
        })
    }

    func updatePopoverLimits() {
        guard let button = statusItem.button, let window = button.window else { return }
        let anchor = window.convertToScreen(button.convert(button.bounds, to: nil))
        // A menu bar can be on a different display from NSScreen.main.
        popoverScreen = NSScreen.screens.first { $0.frame.contains(CGPoint(x: anchor.midX, y: anchor.midY)) }
            ?? window.screen ?? NSScreen.main
        guard let screen = popoverScreen else { return }
        let bounds = PopoverGeometry.availableFrame(visibleFrame: screen.visibleFrame, anchor: anchor)
        panelWidth = min(360, max(1, bounds.width - 32))
        panelMaximumHeight = max(1, bounds.height - 32)
        if let hosting = popover.contentViewController as? TrayHostingController {
            hosting.view.layoutSubtreeIfNeeded()
            let fitting = hosting.sizeThatFits(in: CGSize(width: panelWidth, height: panelMaximumHeight))
            popover.contentSize = CGSize(width: panelWidth, height: min(fitting.height, panelMaximumHeight))
        }
    }

    func popoverDidShow(_ notification: Notification) {
        confinePopover()
    }

    func confinePopover() {
        guard popover.isShown, !confiningPopover,
              let window = popover.contentViewController?.view.window,
              let button = statusItem.button, let anchorWindow = button.window,
              let screen = popoverScreen else { return }
        confiningPopover = true
        defer { confiningPopover = false }
        let anchor = anchorWindow.convertToScreen(button.convert(button.bounds, to: nil))
        let bounds = PopoverGeometry.availableFrame(visibleFrame: screen.visibleFrame, anchor: anchor)
        let original = window.frame
        let frame = PopoverGeometry.confined(original, to: bounds)
        if original != frame { window.setFrame(frame, display: true) }
        // Keep one inspectable geometry snapshot; a cropped window screenshot
        // alone cannot prove that the panel actually fits on the display.
        let report: [String: Any] = [
            "anchor": NSStringFromRect(anchor), "screen": NSStringFromRect(screen.frame),
            "visible": NSStringFromRect(bounds), "before": NSStringFromRect(original),
            "window": NSStringFromRect(window.frame), "contained": bounds.contains(window.frame)
        ]
        if let data = try? JSONSerialization.data(withJSONObject: report, options: .sortedKeys),
           data != lastPopoverGeometry {
            let path = DaemonClient.dataDirectory().appendingPathComponent("run/tray-layout.json")
            try? data.write(to: path, options: .atomic)
            lastPopoverGeometry = data
        }
    }
}
