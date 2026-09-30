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

    func testIMEBackspaceAndSoftwareReturn() {
        let keyboard = RemoteKeyboard()
        var deletions = 0
        var text: [String] = []
        var keys: [Int] = []
        keyboard.backspace = { deletions += 1 }
        keyboard.sendText = { text.append($0) }
        keyboard.sendKey = { code, _, down in if down { keys.append(code) } }
        keyboard.setMarkedText("ni", selectedRange: NSRange(location: 2, length: 0))
        XCTAssertNotNil(keyboard.markedTextRange)
        keyboard.deleteBackward()
        XCTAssertEqual(deletions, 0)
        XCTAssertTrue(text.isEmpty)
        keyboard.unmarkText()
        keyboard.deleteBackward()
        XCTAssertEqual(deletions, 1)
        XCTAssertFalse(keyboard.textView(keyboard, shouldChangeTextIn: NSRange(location: 0, length: 0), replacementText: "\n"))
        XCTAssertEqual(keys, [36])
    }
}
