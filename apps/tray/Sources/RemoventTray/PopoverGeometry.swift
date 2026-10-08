import Foundation

enum PopoverGeometry {
    /// AppKit screen coordinates, including displays left of or above the main
    /// display. Keep the entire window below the menu bar and clear of the Dock.
    static func availableFrame(visibleFrame: CGRect, anchor: CGRect) -> CGRect {
        var frame = visibleFrame.insetBy(dx: 8, dy: 8)
        let top = min(frame.maxY, anchor.minY - 8)
        if top > frame.minY { frame.size.height = top - frame.minY }
        return frame
    }

    static func confined(_ frame: CGRect, to bounds: CGRect) -> CGRect {
        let size = CGSize(width: min(frame.width, bounds.width),
                          height: min(frame.height, bounds.height))
        return CGRect(x: min(max(frame.minX, bounds.minX), bounds.maxX - size.width),
                      y: min(max(frame.minY, bounds.minY), bounds.maxY - size.height),
                      width: size.width, height: size.height)
    }
}
