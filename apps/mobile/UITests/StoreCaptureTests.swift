import XCTest

/// Native store captures use the opt-in loopback protocol host. No screenshot mocks.
@MainActor
final class StoreCaptureTests: XCTestCase {
    func testStoreEnglish() async throws { try await captureStore(locale: "en-US") }
    func testStoreChinese() async throws { try await captureStore(locale: "zh-Hans") }

    private func captureStore(locale: String) async throws {
        let zh = locale == "zh-Hans"
        let name: (String, String) -> String = { zh ? $1 : $0 }
        let app = XCUIApplication()
        XCUIDevice.shared.orientation = .portrait
        app.launchArguments = ["--ui-testing", "--ui-testing-fresh", "-AppleLanguages", zh ? "(zh-Hans)" : "(en)", "-AppleLocale", zh ? "zh_CN" : "en_US"]
        app.launch()
        app.buttons["addConnection"].tap()
        let computerName = app.textFields["connectionName"]
        computerName.tap(); computerName.typeText(name("Studio Mac", "工作室 Mac"))
        let host = app.textFields["connectionHost"]
        XCTAssertTrue(host.waitForExistence(timeout: 5))
        host.tap(); host.typeText("127.0.0.1")
        let port = app.textFields["connectionPort"]
        port.tap(); port.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: 5) + "48689")
        if app.buttons["dismissKeyboard"].exists { app.buttons["dismissKeyboard"].tap() }
        capture("connection-form", locale: locale)
        let generation = try await status()["pin_generation"] as? Int ?? 0
        app.buttons["connectButton"].tap()
        let trust = app.buttons[name("Trust and connect", "信任并连接")]
        if trust.waitForExistence(timeout: 8) { trust.tap() }
        let pin = app.textFields["pairingPIN"]
        if pin.waitForExistence(timeout: 8) {
            var response = try await status()
            for _ in 0..<30 where (response["pin_generation"] as? Int ?? 0) <= generation {
                try await Task.sleep(for: .milliseconds(100))
                response = try await status()
            }
            guard (response["pin_generation"] as? Int ?? 0) > generation,
                  let value = response["pin"] as? String, value.count == 6 else {
                throw NSError(domain: "StoreCapture", code: 1, userInfo: [NSLocalizedDescriptionKey: "Fresh fixture PIN unavailable"])
            }
            pin.tap(); pin.typeText(value)
            let submit = app.buttons["pairingConnect"]
            XCTAssertTrue(submit.isEnabled)
            submit.tap()
            capture("qa-pairing-after-submit", locale: locale)
            if pin.waitForExistence(timeout: 1) {
                submit.tap()
                capture("qa-pairing-after-second-submit", locale: locale)
            }
        }
        let canvas = app.otherElements["remoteCanvas"]
        let frames = NSPredicate { _, _ in (Int(canvas.value as? String ?? "0") ?? 0) > 0 }
        await fulfillment(of: [expectation(for: frames, evaluatedWith: canvas)], timeout: 25)
        guard (Int(canvas.value as? String ?? "0") ?? 0) > 0 else {
            throw NSError(domain: "StoreCapture", code: 2, userInfo: [NSLocalizedDescriptionKey: "No received remote frame"])
        }
        canvas.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
        capture("remote-screen", locale: locale)
        XCUIDevice.shared.orientation = .landscapeLeft
        try await Task.sleep(for: .seconds(1))
        if app.buttons["showSessionControls"].exists { app.buttons["showSessionControls"].tap() }
        app.buttons["remoteKeyboard"].tap()
        XCTAssertTrue(app.buttons["remoteKey-q"].waitForExistence(timeout: 5))
        app.buttons["remoteKey-h"].tap(); app.buttons["remoteKey-i"].tap()
        capture("landscape-keyboard", locale: locale)
        app.buttons["remoteKeyboard"].tap()
        app.buttons["disconnectButton"].tap()
        XCUIDevice.shared.orientation = .portrait
        XCTAssertTrue(app.buttons["addConnection"].waitForExistence(timeout: 5))
        capture("connections", locale: locale)
        app.buttons[name("Settings", "设置")].tap()
        capture("settings", locale: locale)
        for _ in 0..<5 where !app.descendants(matching: .any)["privacyPolicyLink"].isHittable { app.swipeUp() }
        XCTAssertTrue(app.descendants(matching: .any)["privacyPolicyLink"].isHittable)
        XCTAssertTrue(app.descendants(matching: .any)["supportLink"].exists)
        app.terminate()
        // Verify both appearances without altering the prepared light screenshots.
        app.launchArguments += ["-AppleInterfaceStyle", "Dark", "-UIPreferredContentSizeCategoryName", "UICTContentSizeCategoryAccessibilityXXXL"]
        app.launch(); app.buttons[name("Settings", "设置")].tap()
        for _ in 0..<5 where !app.descendants(matching: .any)["privacyPolicyLink"].isHittable { app.swipeUp() }
        XCTAssertTrue(app.descendants(matching: .any)["privacyPolicyLink"].exists)
        capture("qa-dark-large-text", locale: locale)
        app.terminate()
    }

    private func capture(_ kind: String, locale: String) {
        let item = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        item.name = "store_" + locale + "_" + kind
        item.lifetime = .keepAlways
        add(item)
    }

    private func status() async throws -> [String: Any] {
        let (data, _) = try await URLSession.shared.data(from: URL(string: "http://127.0.0.1:48690")!)
        return try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
    }
}
