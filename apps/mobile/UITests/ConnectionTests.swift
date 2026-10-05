import XCTest

@MainActor
final class ConnectionTests: XCTestCase {
    func testRelayFormUsesSNIAndAccessPasswordWithoutFingerprints() {
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing", "--ui-testing-fresh", "-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launch(); app.buttons["addConnection"].tap()
        let relay = app.switches["useRelay"]
        XCTAssertTrue(relay.waitForExistence(timeout: 5))
        relay.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: 0.5)).tap()
        let sni = app.textFields["relayServerName"]
        for _ in 0..<4 where !sni.isHittable { app.swipeUp() }
        XCTAssertTrue(sni.isHittable)
        XCTAssertFalse(app.textFields["hostFingerprint"].exists)
        let verify = app.switches["verifyRelayCertificate"]
        XCTAssertEqual(verify.value as? String, "1")
        capture(app, "Relay SNI with optional certificate verification")
        verify.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: 0.5)).tap()
        XCTAssertEqual(verify.value as? String, "0")
        XCTAssertTrue(app.staticTexts["The connection is encrypted, but the relay's identity is not verified."].exists)
        XCTAssertFalse(app.buttons["Advanced: certificate fingerprints"].exists)
        let password = app.secureTextFields["connectionPassword"]
        for _ in 0..<4 where !password.isHittable { app.swipeUp() }
        XCTAssertTrue(password.isHittable)
        XCTAssertTrue(app.staticTexts["Relay access password"].exists)
        XCTAssertFalse(app.staticTexts["Controller token"].exists)
        XCTAssertFalse(app.textFields["hostFingerprint"].exists)
        capture(app, "Relay SNI and access password without fingerprints")
    }

    func testFirstConnectionConfirmationCanBeCancelledAndRemembered() async throws {
        let (data, _) = try await URLSession.shared.data(from: URL(string:"http://127.0.0.1:48690")!)
        let fixture = try JSONSerialization.jsonObject(with:data) as! [String:Any]
        guard fixture["auth_mode"] as? String == "none" else { throw XCTSkip("No-auth synthetic fixture required") }
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing", "--ui-testing-fresh", "-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launch(); app.buttons["addConnection"].tap()
        let host = app.textFields["connectionHost"]; host.tap(); host.typeText("127.0.0.1")
        let port = app.textFields["connectionPort"]; port.tap()
        port.typeText(String(repeating:XCUIKeyboardKey.delete.rawValue,count:5)+"48689")
        if app.buttons["dismissKeyboard"].exists { app.buttons["dismissKeyboard"].tap() }
        app.buttons["connectButton"].tap()
        let trust = app.buttons["Trust and connect"]
        XCTAssertTrue(trust.waitForExistence(timeout:8))
        capture(app, "Confirm first computer connection")
        app.alerts.buttons["Cancel"].tap()
        let reconnect = app.buttons["Reconnect"]
        XCTAssertTrue(reconnect.waitForExistence(timeout:5)); reconnect.tap()
        XCTAssertTrue(trust.waitForExistence(timeout:8)); trust.tap()
        let connected = NSPredicate(format:"label == %@", "Connected")
        await fulfillment(of:[expectation(for:connected, evaluatedWith:app.staticTexts["sessionStatus"])], timeout:20)
        app.buttons["disconnectButton"].tap()
        let bookmark = app.buttons.matching(NSPredicate(format:"identifier BEGINSWITH %@", "savedConnection-")).firstMatch
        XCTAssertTrue(bookmark.waitForExistence(timeout:5)); bookmark.tap()
        await fulfillment(of:[expectation(for:connected, evaluatedWith:app.staticTexts["sessionStatus"])], timeout:20)
        XCTAssertFalse(trust.exists)
        app.buttons["disconnectButton"].tap()
    }

    func testConsecutiveRelayAndComputerTrustPrompts() async throws {
        let (data, _) = try await URLSession.shared.data(from:URL(string:"http://127.0.0.1:48690")!)
        let fixture = try JSONSerialization.jsonObject(with:data) as! [String:Any]
        guard fixture["relay"] as? Bool == true else { throw XCTSkip("Local relay fixture required") }
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing", "--ui-testing-fresh", "-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launch(); app.buttons["addConnection"].tap()
        app.textFields["connectionHost"].tap(); app.textFields["connectionHost"].typeText("office")
        app.buttons["dismissKeyboard"].tap()
        let relay = app.switches["useRelay"]
        relay.coordinate(withNormalizedOffset: CGVector(dx:0.9, dy:0.5)).tap()
        let endpoint = app.textFields["relayEndpoint"]
        for _ in 0..<4 where !endpoint.isHittable { app.swipeUp() }
        endpoint.tap(); endpoint.typeText("removent://127.0.0.1:48691")
        app.buttons["dismissKeyboard"].tap()
        let transport = app.buttons["relayTransport"]
        for _ in 0..<4 where !transport.isHittable { app.swipeUp() }
        transport.tap(); app.buttons["VPS / QUIC"].tap()
        let password = app.secureTextFields["connectionPassword"]
        for _ in 0..<4 where !password.isHittable { app.swipeUp() }
        password.tap(); password.typeText(String(repeating:"22", count:32))
        app.buttons["dismissKeyboard"].tap(); app.buttons["connectButton"].tap()
        XCTAssertTrue(app.alerts.staticTexts["Trust this relay?"].waitForExistence(timeout:10))
        capture(app, "First relay trust confirmation")
        app.alerts.buttons["Trust and connect"].tap()
        XCTAssertTrue(app.alerts.staticTexts["Trust this computer?"].waitForExistence(timeout:10))
        capture(app, "Computer trust after relay confirmation")
        app.alerts.buttons["Cancel"].tap()
        let reconnect = app.buttons["Reconnect"]
        XCTAssertTrue(reconnect.waitForExistence(timeout:5)); reconnect.tap()
        XCTAssertTrue(app.alerts.staticTexts["Trust this computer?"].waitForExistence(timeout:10))
        XCTAssertFalse(app.alerts.staticTexts["Trust this relay?"].exists)
        app.alerts.buttons["Trust and connect"].tap()
        let connected = NSPredicate(format:"label == %@", "Connected")
        await fulfillment(of:[expectation(for:connected, evaluatedWith:app.staticTexts["sessionStatus"])], timeout:20)
        app.buttons["disconnectButton"].tap()
        let saved = app.buttons.matching(NSPredicate(format:"identifier BEGINSWITH %@", "savedConnection-")).firstMatch
        XCTAssertTrue(saved.waitForExistence(timeout:5)); saved.tap()
        await fulfillment(of:[expectation(for:connected, evaluatedWith:app.staticTexts["sessionStatus"])], timeout:20)
        XCTAssertFalse(app.alerts.firstMatch.exists)
        app.buttons["disconnectButton"].tap()
    }

    func testConnectByInvitationCodeVideoInputAndRememberedReconnect() async throws {
        let url = URL(string: "http://127.0.0.1:48690")!
        let (data, _) = try await URLSession.shared.data(from: url)
        let initial = try JSONSerialization.jsonObject(with: data) as! [String:Any]
        let code = try XCTUnwrap(initial["invitation"] as? String)
        XCTAssertEqual(code.count, 12)
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing", "--ui-testing-fresh", "-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launch(); app.buttons["addConnection"].tap()
        let host = app.textFields["connectionHost"]
        host.tap(); host.typeText(code)
        XCTAssertFalse(app.textFields["connectionPort"].exists)
        if app.buttons["dismissKeyboard"].exists { app.buttons["dismissKeyboard"].tap() }
        self.capture(app, "Connect by invitation code")
        app.buttons["connectButton"].tap()
        let trust = app.buttons["Trust and connect"]
        if trust.waitForExistence(timeout: 5) { capture(app, "First connection confirmation"); trust.tap() }
        let status = app.staticTexts["sessionStatus"]
        let connected = NSPredicate(format: "label == %@", "Connected")
        await fulfillment(of: [expectation(for: connected, evaluatedWith: status)], timeout: 25)
        XCTAssertFalse(app.textFields["pairingPIN"].exists)
        let canvas = app.otherElements["remoteCanvas"]
        let frames = NSPredicate { _, _ in (Int(canvas.value as? String ?? "0") ?? 0) > 3 }
        await fulfillment(of: [expectation(for: frames, evaluatedWith: canvas)], timeout: 15)
        canvas.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap(); app.buttons["Esc"].tap()
        self.capture(app, "Invitation paired desktop")
        let (finalData, _) = try await URLSession.shared.data(from: url)
        let final = try JSONSerialization.jsonObject(with: finalData) as! [String:Any]
        XCTAssertTrue(final["invitation"] is NSNull, "Successful authentication consumes invitation")
        XCTAssertGreaterThan(final["inputs"] as? Int ?? 0, initial["inputs"] as? Int ?? 0)
        XCUIDevice.shared.press(.home); app.activate()
        let reconnect = app.buttons["Reconnect"]
        XCTAssertTrue(reconnect.waitForExistence(timeout: 5)); reconnect.tap()
        await fulfillment(of: [expectation(for: connected, evaluatedWith: status)], timeout: 20)
        XCTAssertFalse(app.textFields["pairingPIN"].exists, "Remembered controller must reconnect using resolved destination")
        app.buttons["disconnectButton"].tap()
        XCTAssertTrue(app.buttons["addConnection"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.staticTexts[code].exists, "One-time code must not appear as a saved destination")
        let destination = app.staticTexts.matching(NSPredicate(format: "label ENDSWITH %@", ":48689")).firstMatch
        XCTAssertTrue(destination.waitForExistence(timeout: 5), "Remembering the computer must save its resolved address")
        app.buttons.containing(.staticText, identifier: destination.label).firstMatch.tap()
        await fulfillment(of: [expectation(for: connected, evaluatedWith: status)], timeout: 20)
        XCTAssertFalse(app.textFields["pairingPIN"].exists)
        app.buttons["disconnectButton"].tap()
    }

    func testCloudSyncConfigurationAndOptOut() {
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing", "--ui-testing-fresh", "-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
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
        app.launchArguments = ["--ui-testing", "--ui-testing-fresh", "-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
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
        app.launchArguments = ["--ui-testing", "--ui-testing-fresh", "-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
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
        try await rvpVideoAndInput()
    }

    func testRVPVideoAndInputAtMaximumDynamicType() async throws {
        try await rvpVideoAndInput(contentSize: "UICTContentSizeCategoryAccessibilityXXXL")
    }

    private func rvpVideoAndInput(contentSize: String? = nil) async throws {
        // Start `cargo run -p removent-host --example mobile_fixture` first.
        let url = URL(string:"http://127.0.0.1:48690")!
        guard let (data,_) = try? await URLSession.shared.data(from:url),
            let initial = try JSONSerialization.jsonObject(with:data) as? [String:Any] else {
            throw XCTSkip("Synthetic RVP fixture is not running")
        }
        let before = initial["inputs"] as? Int ?? 0
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing", "--ui-testing-fresh", "-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        if let contentSize { app.launchArguments += ["-UIPreferredContentSizeCategoryName", contentSize] }
        app.launch()
        app.buttons["addConnection"].tap()
        let host = app.textFields["connectionHost"]
        XCTAssertTrue(host.waitForExistence(timeout:5)); host.tap(); host.typeText("127.0.0.1")
        if contentSize != nil, app.buttons["dismissKeyboard"].exists { app.buttons["dismissKeyboard"].tap() }
        let port = app.textFields["connectionPort"]
        if contentSize != nil {
            let form = app.collectionViews.containing(.textField, identifier: "connectionPort").firstMatch
            // A partly visible field can be hittable while the trailing tap
            // point is covered by the navigation bar or fixed Connect footer.
            for _ in 0..<8 {
                let top = app.navigationBars["Connection"].frame.maxY + 16
                let bottom = app.buttons["connectButton"].frame.minY - 16
                let field = port.frame
                if field.minY >= top && field.maxY <= bottom { break }
                let delta = min(100, max(-100, (top + bottom) / 2 - field.midY))
                let start = form.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
                start.press(forDuration: 0.05, thenDragTo: start.withOffset(CGVector(dx: 0, dy: delta)))
            }
        }
        port.coordinate(withNormalizedOffset: CGVector(dx: 0.98, dy: contentSize == nil ? 0.5 : 0.85)).tap()
        port.typeText(String(repeating:XCUIKeyboardKey.delete.rawValue,count:5) + "48689")
        XCTAssertEqual(port.value as? String, "48689")
        if contentSize != nil {
            if app.buttons["dismissKeyboard"].exists { app.buttons["dismissKeyboard"].tap() }
            for _ in 0..<5 where !app.buttons["connectButton"].isHittable { app.swipeUp() }
        }
        app.buttons["connectButton"].tap()
        let trust = app.buttons["Trust and connect"]
        XCTAssertTrue(trust.waitForExistence(timeout: 8)); trust.tap()
        let pinField = app.textFields["pairingPIN"]
        if pinField.waitForExistence(timeout:12) {
            let (data,_) = try await URLSession.shared.data(from:url)
            let status = try JSONSerialization.jsonObject(with:data) as! [String:Any]
            let pin = try XCTUnwrap(status["pin"] as? String)
            XCTAssertEqual(pin.count,6); pinField.tap(); pinField.typeText(pin); app.buttons["Connect"].tap()
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
        app.buttons["remoteKey-t"].tap()
        let (letterData, _) = try await URLSession.shared.data(from: url)
        let letterStatus = try JSONSerialization.jsonObject(with: letterData) as! [String: Any]
        XCTAssertEqual(letterStatus["typed_text"] as? String, (initial["typed_text"] as? String ?? "") + "t", "The first letter reaches the host before any word is committed")
        for letter in ["e", "s", "t"] { app.buttons["remoteKey-\(letter)"].tap() }
        app.buttons["remoteReturn"].tap()
        app.buttons["remoteKeyboard"].tap()
        let capture = XCTAttachment(screenshot:XCUIScreen.main.screenshot()); capture.name = "RVP mobile connected"
        capture.lifetime = .keepAlways; add(capture)
        try await Task.sleep(for:.milliseconds(300))
        let (finalData,_) = try await URLSession.shared.data(from:url)
        let final = try JSONSerialization.jsonObject(with:finalData) as! [String:Any]
        XCTAssertGreaterThanOrEqual(final["inputs"] as? Int ?? 0, before + 8)
        XCTAssertEqual(final["typed_text"] as? String, (initial["typed_text"] as? String ?? "") + "test")
        XCUIDevice.shared.press(.home)
        app.activate()
        let reconnect = app.buttons["Reconnect"]
        XCTAssertTrue(reconnect.waitForExistence(timeout: 5))
        reconnect.tap()
        await fulfillment(of: [expectation(for: connected, evaluatedWith: status)], timeout: 20)
        await fulfillment(of: [expectation(for: frames, evaluatedWith: canvas)], timeout: 15)
        XCUIDevice.shared.orientation = .landscapeLeft
        try await Task.sleep(for: .seconds(1))
        let fullFrame = canvas.frame
        XCTAssertEqual(fullFrame.width, app.frame.width, accuracy: 1)
        XCTAssertEqual(fullFrame.height, app.frame.height, accuracy: 1)
        self.capture(app, "RVP landscape after reconnect")
        let reveal = app.buttons["showSessionControls"]
        XCTAssertTrue(reveal.waitForExistence(timeout: 6), "Session controls fade after idle")
        XCTAssertFalse(app.buttons["remoteKeyboard"].isHittable)
        reveal.tap()
        canvas.pinch(withScale: 1.25, velocity: 1)
        let picture = canvas.images.firstMatch
        let zoomedFrame = picture.frame
        XCTAssertGreaterThan(zoomedFrame.width, fullFrame.width)
        if reveal.exists { reveal.tap() }
        XCTAssertLessThan(app.buttons["remoteKeyboard"].frame.width, 80, "Keyboard accessibility target must cover its own button")
        app.buttons["remoteKeyboard"].tap()
        XCTAssertTrue(app.buttons["remoteKey-q"].waitForExistence(timeout: 5))
        XCTAssertEqual(canvas.frame, fullFrame, "Landscape keyboard must overlay without shrinking the canvas")
        XCTAssertEqual(picture.frame, zoomedFrame, "Opening the keyboard must preserve picture scale and position")
        XCTAssertFalse(app.buttons["Next keyboard"].exists)
        app.buttons["remoteKey-q"].tap()
        app.buttons["remoteKeyboardPage"].tap()
        app.buttons["remoteKey-1"].tap()
        app.buttons["remoteKeyboardPage"].tap()
        app.buttons["remoteBackspace"].tap()
        app.buttons["Control"].tap()
        app.buttons["remoteShift"].tap()
        app.buttons["remoteKey-Q"].tap()
        app.buttons["remoteKeyboardPage"].tap()
        app.buttons["remoteKey-?"].tap()
        let (shortcutData, _) = try await URLSession.shared.data(from: url)
        let shortcutStatus = try JSONSerialization.jsonObject(with: shortcutData) as! [String: Any]
        let shortcuts = try XCTUnwrap(shortcutStatus["keys"] as? [[String: Any]])
        XCTAssertEqual(shortcuts.suffix(4).compactMap { $0["code"] as? Int }, [12, 12, 44, 44])
        XCTAssertEqual(shortcuts.suffix(4).compactMap { $0["modifiers"] as? Int }, [6, 0, 6, 0], "Ctrl + uppercase/symbol keys must retain Shift")
        XCTAssertEqual(shortcuts.suffix(4).compactMap { $0["down"] as? Bool }, [true, false, true, false])
        app.buttons["Control"].tap()
        app.buttons["remoteKeyboardPage"].tap()
        app.buttons["remoteShift"].tap()
        self.capture(app, "RVP landscape English keyboard without canvas resize")
        app.buttons["hideRemoteKeyboard"].tap()
        XCTAssertTrue(app.buttons["remoteKey-q"].waitForNonExistence(timeout: 5))
        XCTAssertEqual(canvas.frame, fullFrame)
        XCTAssertEqual(picture.frame, zoomedFrame, "Closing the keyboard must preserve picture scale and position")
        // Keyboard dismissal can take longer than the idle timeout on iPad.
        // Reveal the controls through the same affordance used by a person.
        if app.buttons["showSessionControls"].exists { app.buttons["showSessionControls"].tap() }
        XCTAssertTrue(app.buttons["remoteKeyboard"].isHittable)
        app.buttons["remoteKeyboard"].tap()
        XCTAssertTrue(app.buttons["remoteKey-q"].waitForExistence(timeout: 5))
        XCUIDevice.shared.orientation = .portrait
        XCTAssertTrue(app.buttons["remoteKey-q"].waitForExistence(timeout: 5))
        XCTAssertEqual(canvas.frame.width, app.frame.width, accuracy: 1)
        XCUIDevice.shared.orientation = .landscapeLeft
        XCTAssertTrue(app.buttons["remoteKey-q"].waitForExistence(timeout: 5))
        XCTAssertEqual(canvas.frame, fullFrame, "Rotation with the keyboard open must use the current window")
        app.buttons["sessionMenu"].tap()
        XCTAssertTrue(app.buttons["Done"].waitForExistence(timeout: 5))
        app.buttons["Done"].tap()
        XCTAssertTrue(app.buttons["remoteKey-q"].waitForExistence(timeout: 5), "Closing actions must restore the desired English keyboard")
        app.buttons["sessionMenu"].tap()
        let gestureAction = app.buttons["Touch gestures"]
        let actionList = app.collectionViews["sessionActionsList"]
        XCTAssertTrue(actionList.waitForExistence(timeout: 5))
        try await Task.sleep(for: .seconds(5))
        for _ in 0..<5 where !gestureAction.exists || !gestureAction.isHittable { actionList.swipeUp() }
        self.capture(app, "Session actions after keyboard focus released")
        XCTAssertTrue(gestureAction.isHittable, "The menu must stay usable past the fade timeout while the keyboard was shown")
        gestureAction.tap()
        XCTAssertTrue(app.staticTexts["Click"].waitForExistence(timeout: 5), "Choosing gestures must present its sheet after the actions sheet closes")
        XCTAssertTrue(app.buttons["Done"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["remoteKey-q"].waitForNonExistence(timeout: 5))
        XCUIDevice.shared.press(.home)
        app.activate()
        XCTAssertTrue(reconnect.waitForExistence(timeout: 5), "A disconnected session must dismiss its gesture sheet")
        reconnect.tap()
        await fulfillment(of: [expectation(for: connected, evaluatedWith: status)], timeout: 20)
        XCUIDevice.shared.orientation = .portrait
        canvas.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
        app.buttons["disconnectButton"].tap()
        XCTAssertTrue(app.buttons["addConnection"].waitForExistence(timeout:5))
    }

    func testUnattendedAuthenticationVideoAndInput() async throws {
        let url = URL(string: "http://127.0.0.1:48690")!
        guard let (data, _) = try? await URLSession.shared.data(from: url),
              let initial = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              let mode = initial["auth_mode"] as? String, ["password", "otp", "none"].contains(mode) else {
            throw XCTSkip("Start mobile_fixture with REMOVENT_FIXTURE_AUTH=password, otp or none")
        }
        let before = initial["inputs"] as? Int ?? 0
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing", "--ui-testing-fresh", "-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launch(); app.buttons["addConnection"].tap()
        let host = app.textFields["connectionHost"]
        XCTAssertTrue(host.waitForExistence(timeout: 5)); host.tap(); host.typeText("127.0.0.1")
        let port = app.textFields["connectionPort"]
        port.coordinate(withNormalizedOffset: CGVector(dx: 0.98, dy: 0.5)).tap()
        port.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: 5) + "48689")
        app.buttons["connectButton"].tap()
        let trust = app.buttons["Trust and connect"]
        XCTAssertTrue(trust.waitForExistence(timeout: 8)); trust.tap()
        if mode == "password" {
            let field = app.secureTextFields["authenticationPassword"]
            XCTAssertTrue(field.waitForExistence(timeout: 10)); field.tap(); field.typeText("fixture-password")
            self.capture(app, "Password authentication prompt")
            app.buttons["Connect"].tap()
        } else if mode == "otp" {
            let field = app.textFields["pairingPIN"]
            XCTAssertTrue(field.waitForExistence(timeout: 10))
            let (data, _) = try await URLSession.shared.data(from: url)
            let status = try JSONSerialization.jsonObject(with: data) as! [String: Any]
            field.tap(); field.typeText(try XCTUnwrap(status["otp"] as? String))
            self.capture(app, "OTP authentication prompt")
            app.buttons["Connect"].tap()
        }
        let status = app.staticTexts["sessionStatus"]
        await fulfillment(of: [expectation(for: NSPredicate(format: "label == %@", "Connected"), evaluatedWith: status)], timeout: 20)
        let canvas = app.otherElements["remoteCanvas"]
        await fulfillment(of: [expectation(for: NSPredicate { _, _ in (Int(canvas.value as? String ?? "0") ?? 0) > 5 }, evaluatedWith: canvas)], timeout: 15)
        XCTAssertFalse(app.secureTextFields["authenticationPassword"].exists)
        XCTAssertFalse(app.textFields["pairingPIN"].exists)
        canvas.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap(); app.buttons["Esc"].tap()
        try await Task.sleep(for: .milliseconds(300))
        let (finalData, _) = try await URLSession.shared.data(from: url)
        let final = try JSONSerialization.jsonObject(with: finalData) as! [String: Any]
        XCTAssertGreaterThan(final["inputs"] as? Int ?? 0, before)
        self.capture(app, "Unattended \(mode) connected")
        app.buttons["disconnectButton"].tap()
    }

    func testInvalidPortKeepsFormOpen() {
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing", "--ui-testing-fresh", "-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
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
        app.launchArguments = ["--ui-testing", "--ui-testing-fresh", "-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launch()
        capture(app, "Home portrait")
        let welcome = app.cells.containing(.button, identifier: "emptyAddConnection").firstMatch
        XCTAssertTrue(welcome.waitForExistence(timeout: 5))
        // Empty content must leave Nearby visible; it must not expand like a full-screen placeholder.
        XCTAssertLessThan(welcome.frame.height, app.frame.height * 0.35)
        XCTAssertTrue(app.staticTexts["Nearby computers"].isHittable)
        let add = app.buttons["emptyAddConnection"]
        XCTAssertTrue(add.isHittable)
        XCTAssertGreaterThanOrEqual(add.frame.height, 44)
        app.buttons["Settings"].tap()
        capture(app, "Settings portrait")
        app.buttons["Done"].tap()
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
        app.launchArguments = ["--ui-testing", "--ui-testing-fresh", "-AppleLanguages", "(zh-Hans)", "-AppleLocale", "zh_CN"]
        app.launch()
        capture(app, "Home Chinese")
        app.buttons["设置"].tap()
        capture(app, "Settings Chinese")
        app.buttons["完成"].tap()
        app.buttons["addConnection"].tap()
        capture(app, "Connection Chinese")
        XCTAssertTrue(app.buttons["取消"].isHittable)
    }

    private func capture(_ app: XCUIApplication, _ name: String) {
        let capture = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        capture.name = name; capture.lifetime = .keepAlways; add(capture)
    }
}
