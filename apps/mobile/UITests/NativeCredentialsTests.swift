import XCTest

@MainActor
final class NativeCredentialsTests: XCTestCase {
    func testRememberAutomaticallyAuthenticateEditAndRemove() async throws {
        let (data, _) = try await URLSession.shared.data(from:URL(string:"http://127.0.0.1:48690")!)
        let fixture = try JSONSerialization.jsonObject(with:data) as! [String:Any]
        guard fixture["auth_mode"] as? String == "password" else { throw XCTSkip("Password fixture required") }
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing", "--ui-testing-fresh", "-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launch(); app.buttons["addConnection"].tap()
        app.textFields["connectionHost"].tap(); app.textFields["connectionHost"].typeText("127.0.0.1")
        let port = app.textFields["connectionPort"]
        port.tap(); port.typeText(String(repeating:XCUIKeyboardKey.delete.rawValue, count:5) + "48689")
        if app.buttons["dismissKeyboard"].exists { app.buttons["dismissKeyboard"].tap() }
        app.buttons["connectButton"].tap()
        XCTAssertTrue(app.buttons["Trust and connect"].waitForExistence(timeout:10))
        app.buttons["Trust and connect"].tap()
        let password = app.secureTextFields["authenticationPassword"]
        XCTAssertTrue(password.waitForExistence(timeout:10))
        password.tap(); password.typeText("fixture-password")
        let remember = app.switches["rememberNativeCredentials"]
        XCTAssertEqual(remember.value as? String, "0")
        remember.coordinate(withNormalizedOffset:CGVector(dx:0.9, dy:0.5)).tap()
        capture(app, "Remember native password")
        app.buttons["pairingConnect"].tap()
        try await connected(app)
        disconnect(app)
        let bookmark = app.buttons.matching(NSPredicate(format:"identifier BEGINSWITH %@", "savedConnection-")).firstMatch
        XCTAssertTrue(bookmark.waitForExistence(timeout:5)); bookmark.tap()
        try await connected(app)
        XCTAssertFalse(password.exists)
        disconnect(app)

        edit(app, bookmark)
        let stored = app.secureTextFields["nativePassword-password"]
        for _ in 0..<5 where !stored.isHittable { app.swipeUp() }
        XCTAssertTrue(stored.waitForExistence(timeout:5)); XCTAssertTrue(stored.isHittable)
        capture(app, "Native credential editor")
        stored.tap(); stored.typeText(String(repeating:XCUIKeyboardKey.delete.rawValue, count:32) + "wrong-password")
        if app.buttons["dismissKeyboard"].exists { app.buttons["dismissKeyboard"].tap() }
        app.buttons["saveConnectionButton"].tap()
        XCTAssertTrue(bookmark.waitForExistence(timeout:5)); bookmark.tap()
        let rejected = app.staticTexts["Saved credentials were rejected. Close this session and edit the connection to update them."]
        XCTAssertTrue(rejected.waitForExistence(timeout:15)); XCTAssertFalse(password.exists)
        capture(app, "Saved authentication failure recovery")
        app.buttons["disconnectButton"].tap()

        edit(app, bookmark)
        for _ in 0..<5 where !stored.isHittable { app.swipeUp() }
        stored.tap(); stored.typeText(String(repeating:XCUIKeyboardKey.delete.rawValue, count:32) + "fixture-password")
        if app.buttons["dismissKeyboard"].exists { app.buttons["dismissKeyboard"].tap() }
        app.buttons["connectButton"].tap()
        try await connected(app)
        disconnect(app)

        edit(app, bookmark)
        let keep = app.switches["nativeRemember-password"]
        for _ in 0..<5 where !keep.isHittable { app.swipeUp() }
        keep.coordinate(withNormalizedOffset:CGVector(dx:0.9, dy:0.5)).tap()
        app.buttons["saveConnectionButton"].tap()
        XCTAssertTrue(bookmark.waitForExistence(timeout:5)); bookmark.tap()
        XCTAssertTrue(password.waitForExistence(timeout:10))
        XCTAssertEqual(app.switches["rememberNativeCredentials"].value as? String, "0")
        app.buttons["disconnectButton"].tap()
    }

    private func edit(_ app: XCUIApplication, _ bookmark: XCUIElement) {
        XCTAssertTrue(bookmark.waitForExistence(timeout:5))
        bookmark.press(forDuration:1)
        app.buttons["Edit"].tap()
        XCTAssertTrue(app.buttons["saveConnectionButton"].waitForExistence(timeout:5))
    }
    private func connected(_ app: XCUIApplication) async throws {
        await fulfillment(of:[expectation(for:NSPredicate(format:"label == %@", "Connected"),
            evaluatedWith:app.staticTexts["sessionStatus"])], timeout:20)
        let canvas = app.otherElements["remoteCanvas"]
        await fulfillment(of:[expectation(for:NSPredicate { _, _ in
            (Int(canvas.value as? String ?? "0") ?? 0) > 5
        }, evaluatedWith:canvas)], timeout:15)
    }
    private func disconnect(_ app: XCUIApplication) {
        if app.buttons["showSessionControls"].isHittable { app.buttons["showSessionControls"].tap() }
        app.buttons["disconnectButton"].tap()
    }
    private func capture(_ app: XCUIApplication, _ name: String) {
        let attachment = XCTAttachment(screenshot:app.screenshot())
        attachment.name = name; attachment.lifetime = .keepAlways; add(attachment)
    }
}
