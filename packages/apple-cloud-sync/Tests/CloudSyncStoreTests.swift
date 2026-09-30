import XCTest
import CloudKit
@testable import RemoventCloudSync

@MainActor
final class CloudSyncStoreTests: XCTestCase {
    private func fixture() -> [String: Any] {
        ["id": String(repeating: "a", count: 32), "payload": ["version": 1, "revision": String(repeating: "b", count: 32),
            "data": ["name": "Office", "protocol": "vnc", "host": "office.local", "port": 5900,
                     "username": "tester", "domain": "", "relay": NSNull()]], "system_fields": NSNull()]
    }
    func testEncryptedPayloadAndSystemFieldsRoundTrip() async throws {
        let store = CloudSyncStore(call: { _ in [:] }, scope: "test", isCurrent: { true })
        let record = try store.encodeRecord(fixture())
        XCTAssertNil(record["payload"])
        XCTAssertNotNil(record.encryptedValues["payload"])
        let decoded = try store.decodeRecord(record)
        let reencoded = try store.encodeRecord(decoded)
        XCTAssertEqual(reencoded.recordID, record.recordID)
        XCTAssertEqual(reencoded.encryptedValues["payload"] as? Data, record.encryptedValues["payload"] as? Data)
    }
    func testWrongZoneOrMismatchedSystemIdentityCannotBeImported() async throws {
        let store = CloudSyncStore(call: { _ in [:] }, scope: "test", isCurrent: { true })
        let wrong = CKRecord(recordType: "SavedConnection", recordID: .init(recordName: "bad", zoneID: .init(zoneName: "Other")))
        XCTAssertThrowsError(try store.decodeRecord(wrong))
        var decoded = try store.decodeRecord(store.encodeRecord(fixture()))
        decoded["id"] = String(repeating: "c", count: 32)
        XCTAssertThrowsError(try store.encodeRecord(decoded))
    }
    func testUnknownSchemaAndFieldsDoNotAdvanceStorage() async throws {
        var calls = 0
        let store = CloudSyncStore(call: { _ in calls += 1; return [:] }, scope: "test", isCurrent: { true })
        let valid = try store.encodeRecord(fixture())
        let invalid = CKRecord(recordType: "SavedConnection", recordID: .init(recordName: String(repeating: "c", count: 32), zoneID: store.zoneID))
        invalid.encryptedValues["payload"] = try JSONSerialization.data(withJSONObject: ["version": 2, "data": NSNull()]) as NSData
        do { try await store.apply([valid, invalid]); XCTFail("Future schema accepted") } catch {}
        XCTAssertEqual(calls, 0)
        var payload = fixture()["payload"] as! [String: Any]; payload["future_required_field"] = "new"
        invalid.encryptedValues["payload"] = try JSONSerialization.data(withJSONObject: payload) as NSData
        XCTAssertThrowsError(try store.decodeRecord(invalid))
    }
    func testAccountEpochRejectsLateCallbacksAndCheckpoints() async throws {
        var current = true; var calls = 0
        let store = CloudSyncStore(call: { _ in calls += 1; return [:] }, scope: "A", isCurrent: { current })
        let record = try store.encodeRecord(fixture())
        current = false
        do { try await store.apply([record]); XCTFail("Account callback accepted") } catch {}
        do { try await store.checkpoint("token"); XCTFail("Account token accepted") } catch {}
        XCTAssertEqual(calls, 0)
    }
    func testAcknowledgementRetainsTheSentBaseRevision() async throws {
        var received: [String: Any] = [:]
        let store = CloudSyncStore(call: { command in received = command; return [:] }, scope: "A", isCurrent: { true })
        let record = try store.encodeRecord(fixture())
        var value = fixture(); value["base_revision"] = "original-base"
        try await store.acknowledge([record], sent: [record.recordID.recordName: value])
        let records = try XCTUnwrap(received["records"] as? [[String: Any]])
        XCTAssertEqual(records[0]["base_revision"] as? String, "original-base")
        XCTAssertEqual(received["scope"] as? String, "A")
    }
}
