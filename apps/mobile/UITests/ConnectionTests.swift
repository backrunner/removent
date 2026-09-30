import XCTest

@MainActor
final class ConnectionTests: XCTestCase {
    func testCloudSyncConfigurationAndOptOut() {
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing", "-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launch(); app.buttons["Settings"].tap()
        let toggle = app.switches["cloudSyncToggle"]
        for _ in 0..<4 where !toggle.isHittable { app.swipeUp() }
        XCTAssertTrue(toggle.waitForExistence(timeout: 5))
        if toggle.value as? String == "1" { toggle.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: 0.5)).tap() }
        toggle.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: 0.5)).tap()
        XCTAssertTrue(app.staticTexts.matching(NSPredicate(format: "identifier == %@ AND label CONTAINS %@", "cloudSyncStatus", "iCloud is unavailable in this build")).firstMatch.waitForExistence(timeout: 8))
        let capture = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        capture.name = "iCloud settings with configuration status"; capture.lifetime = .keepAlways; add(capture)
        toggle.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: 0.5)).tap()
        XCTAssertTrue(app.staticTexts.matching(NSPredicate(format: "identifier == %@ AND label CONTAINS %@", "cloudSyncStatus", "Off")).firstMatch.waitForExistence(timeout: 5))
        app.buttons["Done"].tap()
        XCTAssertTrue(app.buttons["addConnection"].waitForExistence(timeout: 5))
    }

    func testVNCVideoAndInput() async throws { try await compatibility("VNC", port:"5901") }

    private func compatibility(_ proto:String, port:String) async throws {
        let url = URL(string:"http://127.0.0.1:3392")!
        guard let (data,_) = try? await URLSession.shared.data(from:url),
            let initial = try JSONSerialization.jsonObject(with:data) as? [String:Any] else {
            throw XCTSkip("Synthetic compatibility fixtures are not running")
        }
        let before = initial[proto.lowercased()] as? Int ?? 0
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing", "-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launch(); app.buttons["addConnection"].tap(); app.buttons[proto].tap()
        app.switches["saveConnection"].coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: 0.5)).tap()
        let host = app.textFields["connectionHost"]; host.tap(); host.typeText("127.0.0.1")
        let portField = app.textFields["connectionPort"]
        portField.coordinate(withNormalizedOffset: CGVector(dx: 0.98, dy: 0.5)).tap()
        portField.typeText(String(repeating:XCUIKeyboardKey.delete.rawValue,count:4)+port)
        XCTAssertEqual(portField.value as? String, port)
        if proto == "RDP" {
            app.textFields["Username"].tap(); app.textFields["Username"].typeText("testuser")
            app.secureTextFields["Password"].tap(); app.secureTextFields["Password"].typeText("test-password")
            app.buttons["dismissKeyboard"].tap()
            app.swipeUp(); app.switches["allowUntrustedCertificate"].coordinate(withNormalizedOffset:CGVector(dx:0.9,dy:0.5)).tap()
            XCTAssertEqual(app.switches["allowUntrustedCertificate"].value as? String,"1")
        }
        if app.buttons["dismissKeyboard"].exists { app.buttons["dismissKeyboard"].tap() }
        app.buttons["connectButton"].tap()
        let status = app.staticTexts["sessionStatus"]
        await fulfillment(of:[expectation(for:NSPredicate(format:"label == %@","Connected"),evaluatedWith:status)],timeout:35)
        let canvas = app.otherElements["remoteCanvas"]
        await fulfillment(of:[expectation(for:NSPredicate { _,_ in (Int(canvas.value as? String ?? "0") ?? 0)>3 },evaluatedWith:canvas)],timeout:15)
        canvas.coordinate(withNormalizedOffset:CGVector(dx:0.5,dy:0.5)).tap(); app.buttons["Esc"].tap()
        let screenshot = XCTAttachment(screenshot:XCUIScreen.main.screenshot()); screenshot.name = "\(proto) mobile connected"
        screenshot.lifetime = .keepAlways; add(screenshot)
        try await Task.sleep(for:.milliseconds(300))
        let (finalData,_) = try await URLSession.shared.data(from:url)
        let final = try JSONSerialization.jsonObject(with:finalData) as! [String:Any]
        XCTAssertGreaterThan(final[proto.lowercased()] as? Int ?? 0,before)
        app.buttons["disconnectButton"].tap()
    }
    func testManualConnectionValidationAndCancellation() {
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing", "-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launch()
        app.buttons["addConnection"].tap()
        let host = app.textFields["connectionHost"]
        XCTAssertTrue(host.waitForExistence(timeout:5))
        host.tap(); host.typeText("127.0.0.1")
        // Cancel is available at every stage of the connection form.
        app.buttons["Cancel"].tap()
        XCTAssertTrue(app.buttons["addConnection"].waitForExistence(timeout:5))
    }

    func testRVPVideoAndInput() async throws {
        // Start `cargo run -p removent-host --example mobile_fixture` first.
        let url = URL(string:"http://127.0.0.1:48690")!
        guard let (data,_) = try? await URLSession.shared.data(from:url),
            let initial = try JSONSerialization.jsonObject(with:data) as? [String:Any] else {
            throw XCTSkip("Synthetic RVP fixture is not running")
        }
        let before = initial["inputs"] as? Int ?? 0
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing", "-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launch()
        app.buttons["addConnection"].tap()
        let host = app.textFields["connectionHost"]
        XCTAssertTrue(host.waitForExistence(timeout:5)); host.tap(); host.typeText("127.0.0.1")
        let port = app.textFields["connectionPort"]
        port.coordinate(withNormalizedOffset: CGVector(dx: 0.98, dy: 0.5)).tap()
        port.typeText(String(repeating:XCUIKeyboardKey.delete.rawValue,count:5) + "48689")
        app.buttons["connectButton"].tap()
        let pinField = app.textFields["pairingPIN"]
        if pinField.waitForExistence(timeout:12) {
            let (data,_) = try await URLSession.shared.data(from:url)
            let status = try JSONSerialization.jsonObject(with:data) as! [String:Any]
            let pin = try XCTUnwrap(status["pin"] as? String)
            XCTAssertEqual(pin.count,6); pinField.tap(); pinField.typeText(pin); app.buttons["Pair"].tap()
        }
        let status = app.staticTexts["sessionStatus"]
        let connected = NSPredicate(format:"label == %@", "Connected")
        await fulfillment(of:[expectation(for:connected,evaluatedWith:status)],timeout:20)
        let canvas = app.otherElements["remoteCanvas"]
        let frames = NSPredicate { _,_ in (Int(canvas.value as? String ?? "0") ?? 0) > 5 }
        await fulfillment(of:[expectation(for:frames,evaluatedWith:canvas)],timeout:15)
        canvas.coordinate(withNormalizedOffset:CGVector(dx:0.5,dy:0.5)).tap()
        app.buttons["Esc"].tap()
        app.buttons["remoteKeyboard"].tap()
        app.typeText("test\n")
        app.buttons["remoteKeyboard"].tap()
        let capture = XCTAttachment(screenshot:XCUIScreen.main.screenshot()); capture.name = "RVP mobile connected"
        capture.lifetime = .keepAlways; add(capture)
        try await Task.sleep(for:.milliseconds(300))
        let (finalData,_) = try await URLSession.shared.data(from:url)
        let final = try JSONSerialization.jsonObject(with:finalData) as! [String:Any]
        XCTAssertGreaterThanOrEqual(final["inputs"] as? Int ?? 0, before + 8)
        XCUIDevice.shared.press(.home)
        app.activate()
        let reconnect = app.buttons["Reconnect"]
        XCTAssertTrue(reconnect.waitForExistence(timeout: 5))
        reconnect.tap()
        await fulfillment(of: [expectation(for: connected, evaluatedWith: status)], timeout: 20)
        await fulfillment(of: [expectation(for: frames, evaluatedWith: canvas)], timeout: 15)
        XCUIDevice.shared.orientation = .landscapeLeft
        try await Task.sleep(for: .seconds(1))
        self.capture(app, "RVP landscape after reconnect")
        XCUIDevice.shared.orientation = .portrait
        app.buttons["disconnectButton"].tap()
        XCTAssertTrue(app.buttons["addConnection"].waitForExistence(timeout:5))
    }

    func testInvalidPortKeepsFormOpen() {
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing", "-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launch(); app.buttons["addConnection"].tap()
        XCTAssertFalse(app.buttons["connectButton"].isEnabled)
        let host = app.textFields["connectionHost"]; host.tap(); host.typeText("127.0.0.1")
        let port = app.textFields["connectionPort"]
        port.coordinate(withNormalizedOffset: CGVector(dx: 0.98, dy: 0.5)).tap()
        port.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: 5) + "0")
        XCTAssertEqual(port.value as? String, "0")
        app.buttons["connectButton"].tap()
        XCTAssertTrue(app.staticTexts["connectionError"].waitForExistence(timeout: 3))
        XCTAssertTrue(host.exists)
        XCTAssertFalse(app.buttons["disconnectButton"].exists)
        app.buttons["Cancel"].tap()
    }

    func testLayoutsAndDynamicType() {
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing", "-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launch()
        capture(app, "Home portrait")
        app.buttons["addConnection"].tap()
        capture(app, "Connection portrait")
        XCUIDevice.shared.orientation = .landscapeLeft
        Thread.sleep(forTimeInterval: 1)
        capture(app, "Connection landscape")
        XCTAssertTrue(app.buttons["Cancel"].isHittable)
        XCUIDevice.shared.orientation = .portrait
        Thread.sleep(forTimeInterval: 1)
        app.terminate()
        app.launchArguments += ["-UIPreferredContentSizeCategoryName", "UICTContentSizeCategoryAccessibilityXXXL"]
        app.launch(); app.buttons["addConnection"].tap()
        capture(app, "Connection accessibility text")
        XCTAssertTrue(app.buttons["Cancel"].isHittable)
        XCTAssertTrue(app.buttons["connectButton"].exists)
        app.buttons["Cancel"].tap()
        app.terminate()
        app.launchArguments = ["--ui-testing", "-AppleLanguages", "(zh-Hans)", "-AppleLocale", "zh_CN"]
        app.launch(); app.buttons["addConnection"].tap()
        capture(app, "Connection Chinese")
        XCTAssertTrue(app.buttons["取消"].isHittable)
    }

    private func capture(_ app: XCUIApplication, _ name: String) {
        let capture = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        capture.name = name; capture.lifetime = .keepAlways; add(capture)
    }
}
