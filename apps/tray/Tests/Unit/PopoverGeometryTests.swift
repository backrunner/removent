import XCTest
@testable import RemoventTray

final class PopoverGeometryTests: XCTestCase {
    func testRightEdgeAndTopStayInsideVisibleDisplay() {
        let bounds = PopoverGeometry.availableFrame(
            visibleFrame: CGRect(x: 0, y: 70, width: 1440, height: 800),
            anchor: CGRect(x: 1410, y: 870, width: 24, height: 24))
        let result = PopoverGeometry.confined(CGRect(x: 1260, y: 550, width: 380, height: 480), to: bounds)
        XCTAssertTrue(bounds.contains(result))
        XCTAssertEqual(result.maxX, 1432)
        XCTAssertEqual(result.maxY, 862)
    }

    func testSecondaryDisplayUsesItsOwnNegativeOrigin() {
        let bounds = PopoverGeometry.availableFrame(
            visibleFrame: CGRect(x: -1920, y: 240, width: 1920, height: 1056),
            anchor: CGRect(x: -1900, y: 1296, width: 24, height: 24))
        let result = PopoverGeometry.confined(CGRect(x: -2080, y: 900, width: 380, height: 500), to: bounds)
        XCTAssertTrue(bounds.contains(result))
        XCTAssertEqual(result.minX, -1912)
        XCTAssertEqual(result.maxY, 1288)
    }

    func testSmallDisplayCapsWindowSize() {
        let bounds = PopoverGeometry.availableFrame(
            visibleFrame: CGRect(x: 0, y: 60, width: 640, height: 396),
            anchor: CGRect(x: 300, y: 456, width: 24, height: 24))
        let result = PopoverGeometry.confined(CGRect(x: 150, y: -300, width: 700, height: 900), to: bounds)
        XCTAssertEqual(result, bounds)
    }

    func testAlreadyVisibleWindowDoesNotMove() {
        let bounds = CGRect(x: 8, y: 8, width: 1400, height: 850)
        let frame = CGRect(x: 750, y: 400, width: 380, height: 440)
        XCTAssertEqual(PopoverGeometry.confined(frame, to: bounds), frame)
    }
}
