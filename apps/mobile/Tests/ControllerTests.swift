import XCTest
@testable import RemoventMobile

@MainActor
final class ControllerTests: XCTestCase {
    func testCloudImportKeepsCredentialsLocalAndSeparatesAccounts() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let model = ControllerModel(storageDirectory: directory, startServices: false)
        defer { try? FileManager.default.removeItem(at: directory) }
        func sync(_ command: [String: Any]) throws { try model.call(["op": "sync", "command": command]) }
        try sync(["op": "enable", "enabled": true])
        try sync(["op": "bind", "scope": "test/account-A"])
        let record: [String: Any] = ["id": String(repeating: "a", count: 32), "system_fields": NSNull(), "payload": [
            "version": 1, "revision": String(repeating: "b", count: 32), "data": [
                "name": "Cloud computer", "protocol": "vnc", "host": "test.invalid", "port": 5900,
                "username": "", "domain": "", "relay": NSNull()]]]
        try sync(["op": "apply", "scope": "test/account-A", "records": [record]])
        try model.reload()
        let bookmark = try XCTUnwrap(model.bookmarks.first)
        XCTAssertEqual(bookmark.title, "Cloud computer")
        XCTAssertEqual(bookmark.credentials_review_required, true)
        XCTAssertFalse(bookmark.password_hint)
        XCTAssertFalse(bookmark.accept_invalid_certificate)
        XCTAssertNil(ConnectionDraft(bookmark).credentialID)
        try sync(["op": "bind", "scope": "test/account-B"])
        try model.reload(); XCTAssertTrue(model.bookmarks.isEmpty)
        XCTAssertThrowsError(try sync(["op": "apply", "scope": "test/account-A", "records": [record]]))
        try sync(["op": "bind", "scope": "test/account-A"])
        try model.reload(); XCTAssertEqual(model.bookmarks.first?.id, bookmark.id)
    }

    func testZoomedCoordinatesAndEdges() {
        XCTAssertEqual(CanvasView.remotePoint(CGPoint(x:150,y:100), imageSize:CGSize(width:300,height:200),
            remoteSize:CGSize(width:1920,height:1080)), CGPoint(x:960,y:540))
        XCTAssertEqual(CanvasView.remotePoint(CGPoint(x:900,y:-10), imageSize:CGSize(width:300,height:200),
            remoteSize:CGSize(width:1920,height:1080)), CGPoint(x:1919,y:0))
        XCTAssertNil(CanvasView.remotePoint(.zero,imageSize:.zero,remoteSize:.zero))
    }
    func testRelayUsesWireTransportAndNoCertificateBypass() throws {
        var draft = ConnectionDraft(); draft.host = "office"; draft.useRelay = true
        draft.relayEndpoint = "removent://relay.example:443"; draft.hostFingerprint = String(repeating:"a",count:64)
        let request = try draft.request(audio:true,clipboard:true)
        XCTAssertEqual((request["relay"] as? [String:String])?["transport"], "websocket")
        XCTAssertEqual(request["accept_invalid_certificate"] as? Bool, false)
    }
    func testKeyboardModifiersAndSpecialKeys() {
        XCTAssertEqual(RemoteKeyboard.modifiers([.command,.shift]),18)
        XCTAssertEqual(RemoteKeyboard.special(.keyboardLeftArrow),123)
        XCTAssertEqual(RemoteKeyboard.macKey(for:"c"),8)
    }

    func testDraftValidationAndSavedPasswordIntent() throws {
        var draft = ConnectionDraft()
        draft.host = "  \n"
        XCTAssertThrowsError(try draft.request(audio: false, clipboard: false))
        draft.host = "  mac.local \n"; draft.port = " 5900 "
        XCTAssertEqual(try draft.request(audio: false, clipboard: false)["host"] as? String, "mac.local")
        draft.port = "65536"
        XCTAssertThrowsError(try draft.request(audio: false, clipboard: false))
        draft.protocol = .rdp; draft.port = "3389"
        XCTAssertThrowsError(try draft.request(audio: false, clipboard: false))
        draft.username = "tester"; draft.useRelay = true
        XCTAssertThrowsError(try draft.request(audio: false, clipboard: false))
    }

    func testEditingNamePreservesKeychainPasswordAndRejectsAnotherHost() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let model = ControllerModel(storageDirectory: directory, startServices: false)
        defer {
            for bookmark in model.bookmarks { model.delete(bookmark) }
            try? FileManager.default.removeItem(at: directory)
        }
        XCTAssertNil(model.error)
        var draft = ConnectionDraft(); draft.protocol = .vnc; draft.host = "test.invalid"
        draft.port = "5900"; draft.password = "synthetic-review-password"
        try model.save(draft)
        var edit = ConnectionDraft(try XCTUnwrap(model.bookmarks.first))
        XCTAssertTrue(edit.hasSavedPassword)
        XCTAssertNotNil(edit.credentialID)
        edit.name = "Renamed computer"
        try model.save(edit)
        XCTAssertTrue(try XCTUnwrap(model.bookmarks.first).password_hint)
        try model.validate(edit)
        edit.host = "another.invalid"
        XCTAssertThrowsError(try model.validate(edit))
        XCTAssertThrowsError(try model.save(edit))
        XCTAssertEqual(model.bookmarks.first?.host, "test.invalid")
        edit.host = "test.invalid"; edit.replaceSavedPassword = true
        XCTAssertNil(edit.credentialID)
        try model.save(edit)
        XCTAssertFalse(try XCTUnwrap(model.bookmarks.first).password_hint)
    }

    func testSessionFailureAndIndependentRefreshCapability() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let model = ControllerModel(storageDirectory: directory, startServices: false)
        defer { model.disconnect(); try? FileManager.default.removeItem(at: directory) }
        model.accept(["type": "progress", "generation": 0, "stage": "NewStageWithoutEllipsis"])
        XCTAssertTrue(model.isConnecting)
        model.accept(["type": "pin", "generation": 0])
        XCTAssertTrue(model.needsPIN); XCTAssertFalse(model.isConnecting)
        model.accept(["type": "ready", "generation": 0, "clipboard": false, "refresh": true])
        XCTAssertTrue(model.ready); XCTAssertTrue(model.canRefresh); XCTAssertFalse(model.remoteClipboard)
        model.onFrame = { _ in }
        model.input(["kind": "key", "code": 53, "modifiers": 0, "down": true])
        XCTAssertFalse(model.ready); XCTAssertFalse(model.isConnecting)
        XCTAssertEqual(model.status, L("Disconnected", "连接已断开"))
        XCTAssertNotNil(model.error)
        XCTAssertNotNil(model.onFrame, "The canvas must stay subscribed for reconnect")
    }

    func testDirectEnglishInputWithoutComposition() {
        let keyboard = RemoteKeyboard()
        var deletions = 0
        var text: [String] = []
        var keys: [Int] = []
        keyboard.backspace = { deletions += 1 }
        keyboard.sendText = { text.append($0) }
        keyboard.sendKey = { code, _, down in if down { keys.append(code) } }
        keyboard.insertText("ni")
        XCTAssertEqual(text, ["n", "i"], "Each letter must be sent immediately")
        keyboard.insertText("中文é🙂")
        XCTAssertEqual(text, ["n", "i"], "Non-ASCII input is rejected")
        keyboard.insertText(" A1!\n\t")
        XCTAssertEqual(text, ["n", "i", " ", "A", "1", "!"])
        keyboard.deleteBackward()
        XCTAssertEqual(deletions, 1)
        XCTAssertEqual(keys, [36, 48])
        XCTAssertTrue(keyboard.inputView is EnglishKeyboardView)
        XCTAssertFalse(keyboard is UITextInput)
    }

    func testHardwareKeyboardUsesEnglishPhysicalKeys() {
        XCTAssertEqual(RemoteKeyboard.englishCharacter(.keyboardA, modifiers: []), "a")
        XCTAssertEqual(RemoteKeyboard.englishCharacter(.keyboardA, modifiers: [.shift]), "A")
        XCTAssertEqual(RemoteKeyboard.englishCharacter(.keyboardA, modifiers: [.alphaShift]), "A")
        XCTAssertEqual(RemoteKeyboard.englishCharacter(.keyboardA, modifiers: [.alphaShift, .shift]), "a")
        XCTAssertEqual(RemoteKeyboard.englishCharacter(.keyboard1, modifiers: [.shift]), "!")
        XCTAssertNil(RemoteKeyboard.englishCharacter(.keyboardEscape, modifiers: []))
    }

    func testShiftedEnglishShortcutsKeepTheirPhysicalKeyAndShift() throws {
        for (character, code) in [("A", 0), ("!", 18), ("?", 44), (":", 41), ("{", 33), ("~", 50)] {
            let key = try XCTUnwrap(RemoteKeyboard.macKeyStroke(for: character.unicodeScalars.first!))
            XCTAssertEqual(key.code, code)
            XCTAssertEqual(key.modifiers, 2, "Ctrl/Option + \(character) must retain Shift")
        }
        XCTAssertEqual(RemoteKeyboard.macKeyStroke(for: "a")?.modifiers, 0)
        for value in 32...126 {
            XCTAssertNotNil(RemoteKeyboard.macKeyStroke(for: Unicode.Scalar(value)!), "Every printable ASCII key must support shortcuts")
        }
        XCTAssertNil(RemoteKeyboard.macKeyStroke(for: "中"))
    }

    func testLandscapeViewportUsesCurrentWindowAcrossKeyboardAndResize() {
        let original = CGSize(width: 874, height: 402)
        XCTAssertEqual(SessionView.viewportSize(available: CGSize(width: 874, height: 382), window: original, ready: true), original)
        let resized = CGSize(width: 874, height: 350)
        XCTAssertEqual(SessionView.viewportSize(available: CGSize(width: 874, height: 330), window: resized, ready: true), resized)
        let portrait = CGSize(width: 402, height: 874)
        let portraitWithKeyboard = CGSize(width: 402, height: 614)
        XCTAssertEqual(SessionView.viewportSize(available: portraitWithKeyboard, window: portrait, ready: true), portraitWithKeyboard)
        XCTAssertEqual(SessionView.viewportSize(available: portrait, window: original, ready: true), portrait, "Do not use a stale window orientation")
        XCTAssertEqual(SessionView.viewportSize(available: .zero, window: original, ready: false), CGSize(width: 1, height: 1))
        XCTAssertEqual(SessionView.viewportSize(available: CGSize(width: CGFloat.nan, height: -20), window: .zero, ready: false), CGSize(width: 1, height: 1))
    }

    func testEnglishKeyboardTitlesAndDirectInputSurvivePageChanges() throws {
        let receiver = RemoteKeyboard()
        var text: [String] = []
        receiver.sendText = { text.append($0) }
        let input = try XCTUnwrap(receiver.inputView)
        func buttons(in view: UIView) -> [UIButton] {
            (view as? UIButton).map { [$0] } ?? view.subviews.flatMap { buttons(in: $0) }
        }
        func press(_ id: String) throws {
            let button = try XCTUnwrap(buttons(in: input).first { $0.accessibilityIdentifier == id })
            XCTAssertFalse(button.configuration?.title?.isEmpty ?? true)
            button.sendActions(for: .primaryActionTriggered)
        }
        try press("remoteShift"); try press("remoteKey-A")
        try press("remoteKeyboardPage"); try press("remoteKey-1")
        try press("remoteShift"); try press("remoteKey-~")
        try press("remoteKeyboardPage"); try press("remoteShift"); try press("remoteKey-z")
        XCTAssertEqual(text, ["A", "1", "~", "z"])
        XCTAssertTrue(buttons(in: input).allSatisfy { !($0.configuration?.title?.isEmpty ?? true) })
    }

    func testLandscapeFillPreservesAspectAndRemoteCoordinates() {
        let remote = CGSize(width: 1920, height: 1080)
        let viewport = CGSize(width: 852, height: 393)
        let fill = CanvasView.displaySize(remoteSize: remote, viewport: viewport, fill: true)
        XCTAssertEqual(fill.width, viewport.width, accuracy: 0.01)
        XCTAssertGreaterThan(fill.height, viewport.height)
        XCTAssertEqual(fill.width / fill.height, remote.width / remote.height, accuracy: 0.001)
        let center = CanvasView.remotePoint(CGPoint(x: fill.width / 2, y: fill.height / 2), imageSize: fill, remoteSize: remote)
        XCTAssertEqual(center, CGPoint(x: 960, y: 540))
        let fit = CanvasView.displaySize(remoteSize: remote, viewport: viewport, fill: false)
        XCTAssertEqual(fit.height, viewport.height, accuracy: 0.01)
        XCTAssertLessThan(fit.width, viewport.width)
    }
}
