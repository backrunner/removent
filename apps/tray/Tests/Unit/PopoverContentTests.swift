import AppKit
import SwiftUI
import XCTest
@testable import RemoventTray

final class PopoverContentTests: XCTestCase {
    @MainActor func testShortContentKeepsNaturalHeight() {
        let view = TrayPopoverContent(width: 360, maximumHeight: 900) {
            Text("Permissions").frame(height: 200)
        }
        let hosting = NSHostingController(rootView: view)
        let size = hosting.sizeThatFits(in: CGSize(width: 360, height: 900))
        XCTAssertEqual(size.width, 360, accuracy: 1)
        XCTAssertEqual(size.height, 200, accuracy: 1)
    }

    @MainActor func testTallContentUsesAvailableHeight() {
        let view = TrayPopoverContent(width: 360, maximumHeight: 280) {
            Text("Sessions").frame(height: 800)
        }
        let hosting = NSHostingController(rootView: view)
        let size = hosting.sizeThatFits(in: CGSize(width: 360, height: 1000))
        XCTAssertEqual(size.width, 360, accuracy: 1)
        XCTAssertEqual(size.height, 280, accuracy: 1)
    }
}
