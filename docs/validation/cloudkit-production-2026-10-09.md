# CloudKit Production validation — 2026-10-09

The installed `0.1.3-beta.3` desktop helper completed real CloudKit Production
record exchange. This used two isolated local data directories on one Mac with
the same iCloud account. **Physical Mac/iPhone convergence remains unverified.**

The tested source revision was
`12248fa52fca15f4e971d8254f5be2a38b921026`, the immutable
[`v0.1.3-beta.3` release](https://github.com/backrunner/removent/releases/tag/v0.1.3-beta.3).
These results supplement the [historical local checks](cloudkit-sync-validation.md).

## Provisioning and deployed schema

- The desktop caller is `com.alkinum.removent.sync`, authorized for CloudKit and
  production push notifications under team `PB8H83VL3Z`.
- The helper has a Developer ID provisioning profile and a matching Developer ID
  signature. Packaging checks the actual certificate against the profile, as well
  as the App ID, team, container, environment and push entitlement.
- Desktop and mobile reuse the existing `iCloud.com.alkinum.removent` container.
  Its identity and the mobile App ID configuration were not changed.
- With user authorization, `SavedConnection.payload` was added as **Encrypted
  Bytes**, without query indexes, and deployed from Development to Production.
  The maintainer completed the final deployment manually; the native record
  exchange below verified that the Production schema accepted the encrypted field.
- The native helper uses the private `RemoventConnections` zone and accesses the
  payload through `CKRecord.encryptedValues`; it does not synchronize passwords,
  Keychain references, pairing secrets or certificate exceptions.

The earlier Production zone fetch returned HTTP 500 / CKError 15/2000. A later
empty synchronization round reached `ready` before schema deployment, and the
record round trip below also succeeded without that error. Schema deployment
was required for connection records; it was not established as the cause of the
earlier HTTP 500.

## Observed Production round trip

The test launched the installed
`/Applications/Removent.app/Contents/Helpers/RemoventSync.app/Contents/MacOS/RemoventSync`
with an isolated `--data-dir` and the test parent's PID. Its normal
`removent-cli cloud-sync` commands accessed that same directory through
`REMOVENT_DATA_DIR`. A local driver used the compiled Rust client's
`SavedConnections::upsert` and `SavedConnections::remove` APIs for edits, so the
normal local transaction and outbox logic participated in the test.

Replica A created a synthetic bookmark for `192.0.2.123:5500`, with no credentials
or relay route. Replica B began with an empty local store. Both replicas bound to
the same account scope, and all six synchronization rounds reached `ready` with
zero pending changes and a recorded successful synchronization timestamp.

| Step | Observed result |
| --- | --- |
| A uploads the initial bookmark | The server acknowledged the encrypted record; its system fields were persisted and the base revision matched the local revision. |
| Fresh B downloads the bookmark | The expected name and endpoint were verified in the imported local bookmark, with no password reference. |
| B changes the name and uploads | The updated record was acknowledged with no pending changes. |
| A downloads B's change | A's bookmark contained the updated name and retained the expected endpoint. |
| A removes the bookmark | The normal client removal queued a logical deletion; its encrypted tombstone was acknowledged by the server. |
| B downloads the logical deletion | The bookmark was absent and the acknowledged tombstone matched the local revision. |

The final successful synchronization timestamps were `1791558787` for A and
`1791558797` for B. The final state was inspected at `2026-10-09T15:14:06Z`.
Both helpers exited after sync was disabled in their test directories. The
synthetic bookmark was absent from both local catalogs; its logical cloud
tombstone remains under the normal retention policy. The isolated directories
were then removed, and the real user's sync preference remained disabled.

## Remaining acceptance

This proves signed native Production record exchange and the shared local
storage path on one Mac. It does not establish:

- Physical Mac/iPhone or Mac/iPad convergence with a mobile distribution build.
- Live endpoint/relay-route changes, concurrent offline conflicts or a different
  iCloud account switch.
- Production push delivery, background mobile wakeup, quota recovery or extended
  network-failure recovery.

The existing local regression suites cover these storage policies separately;
their results do not replace the missing device/server acceptance checks.
