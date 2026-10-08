import AppKit

/// The two display apertures from assets/branding/AppIcon.svg, reduced to a
/// template silhouette so macOS supplies the correct menu-bar contrast.
enum BrandIcon {
    static let menuBar: NSImage = {
        let image = NSImage(size: NSSize(width: 24, height: 20), flipped: false) { _ in
            let transform = NSAffineTransform()
            transform.translateX(by: 12, yBy: 10)
            transform.scale(by: 0.033)
            transform.rotate(byDegrees: 16)
            transform.translateX(by: -509, yBy: -519.5)
            transform.concat()
            NSColor.black.setFill()
            for (x, y) in [(419.0, 426.0), (218.0, 340.0)] {
                let ring = NSBezierPath(roundedRect: NSRect(x: x, y: y, width: 381, height: 273),
                                        xRadius: 93, yRadius: 93)
                ring.append(NSBezierPath(roundedRect: NSRect(x: x + 60, y: y + 60, width: 261, height: 153),
                                         xRadius: 33, yRadius: 33))
                ring.windingRule = .evenOdd
                ring.fill()
            }
            return true
        }
        image.isTemplate = true
        image.accessibilityDescription = "Removent"
        return image
    }()

    static var app: NSImage {
        Bundle.trayResources.url(forResource: "AppIcon", withExtension: "png")
            .flatMap(NSImage.init(contentsOf:)) ?? menuBar
    }
}
