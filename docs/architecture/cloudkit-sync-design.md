# Shared CloudKit connection storage

Implemented on 2026-09-30. The local implementation and build integration are in
place; signed CloudKit access and a real Mac/iPhone exchange still require Apple
account provisioning and live acceptance. Local tests do not establish either.
The apps require macOS 26 and iOS/iPadOS 26 or newer.

## User behavior

Enable **Settings → iCloud → Sync saved connections** on the participating devices.
The default is off. Each user accesses their own private database in
`iCloud.com.alkinum.removent`. The mobile App ID is `com.alkinum.removent.mobile`;
the macOS CloudKit caller is the embedded `com.alkinum.removent.sync` helper.
The desktop GUI keeps `com.alkinum.removent` and its Developer ID distribution.

Names, protocols, endpoints, usernames/domains and relay routes synchronize.
Passwords, certificate exceptions, device keys, pairing approval and host service
settings remain local. Imported VNC/RDP/relay connections open the connection form
for credential review on the receiving device. A synced address does not make an
offline or LAN-only computer reachable.

Both settings screens show status, pending changes, the last successful exchange,
a retry action, and the local/cloud values of conflicting connections. Independent
name and endpoint edits merge automatically; competing edits require a choice.
Recreating a connection deleted remotely gives it a new ID. Turning sync off keeps
local connections and queues further changes for that same account.

Signing out leaves that account's cached connections usable offline and suspends
cloud callbacks. Signing into a different account archives the previous account's
connections/outbox separately before loading the new account. Previous data is
restored only when returning to that account; it is never automatically uploaded
to the new account. Local archives retain Keychain references, not passwords.

## Components

| Component | Responsibility |
| --- | --- |
| [`packages/client/src/cloud_sync/mod.rs`](../../packages/client/src/cloud_sync/mod.rs) | Allowed-field wire format, validation, account ownership, content revisions, three-way merge, conflicts, logical deletion tombstones, acknowledgements and checkpoints. |
| [`packages/client/src/saved/mod.rs`](../../packages/client/src/saved/mod.rs) | Shared transaction boundary for GUI/mobile/CLI edits and sync imports. |
| [`packages/apple-cloud-sync/Sources`](../../packages/apple-cloud-sync/Sources) | Shared encrypted CloudKit record codec, lifecycle/account handling, bounded batches and retry backoff. |
| `CloudKitSyncTransport` | One `CKSyncEngine` transport on both platforms. Explicit fetch-then-send cycles prevent uploading before initial reconciliation. State is persisted only after successfully applying received pages. |
| [`apps/cloud-sync-helper/SyncHelper`](../../apps/cloud-sync-helper/SyncHelper) | User-owned, separately provisioned macOS app bundle. Calls `removent-cli cloud-sync` over bounded stdin/stdout; never accesses Keychain or starts a host identity. |
| [`packages/mobile/src/lib.rs`](../../packages/mobile/src/lib.rs) | Handle-independent `rm_sync` storage ABI, executed off the mobile main thread. |

The helper has one owner per data directory and exits after its parent GUI exits,
its executable bundle is replaced, or sync is disabled. The desktop restarts a
missing helper while enabled. CloudKit never runs in the root/login-window host.
Foreground activation, pending edits, manual retry and silent notifications wake
sync. A five-minute refresh while the app is running provides a backstop; delivery
is eventual, not immediate. Retry-after and exponential delays limit failed calls.

## Persistence and recovery

`connections.json` stores the connection array. `connections-sync.json`
contains account-scoped outboxes, acknowledged versions, archived connections and
the serialized CKSyncEngine state. `connections-transaction.json` is a redo journal
that commits the two files under the existing connection lock. All three are
private local files. Recovery rolls forward an interrupted transaction before
reading another snapshot; a committed password reference is never rolled back by
deleting its newly created Keychain item after a materialization failure.

Cloud records use a `RemoventConnections` custom zone and `SavedConnection` record
type. Each stable connection ID is a record name. The single `payload` field uses
`CKRecord.encryptedValues` and contains a version, random content revision, and
allowed connection fields (or null for a tombstone). Certificate exceptions,
password references, last-used timestamps and device identity are not serialized
into this payload. No query indexes are needed on encrypted data.

Deletion tombstones are retained indefinitely in this first version. Expired tokens
cause a fresh fetch; zone recreation resets transport state without discarding
local changes. A late upload acknowledgement cannot clear newer local edits.
Unsupported schemas/extra payload fields stop the page and preserve its prior
checkpoint instead of silently dropping data. This is deliberately conservative;
future schema additions need an explicit compatibility/migration policy.

## Apple setup and release

The common team must authorize both App IDs for the same container. Container
registration is not performed by writing an entitlement file. The current machine
reported **No Accounts** from Xcode and has no matching Removent provisioning
profiles; the developer portal also required sign-in during this implementation.

1. Sign into the appropriate developer account in Xcode (the release team in this
   repository is `PB8H83VL3Z`). Register/confirm `iCloud.com.alkinum.removent` once.
2. Generate projects with `xcodegen generate --spec apps/mobile/project.yml` and
   `xcodegen generate --spec apps/cloud-sync-helper/project.yml`. Enable CloudKit and push
   notifications for their explicit App IDs and generate authorized development
   profiles. Both projects declare the same container and environment settings.
3. Run development builds on a signed-in Mac and iPhone to create/verify the
   `SavedConnection.payload` encrypted-bytes schema. Deploy that schema to Production
   using CloudKit Console before distributing release/TestFlight builds. Keep
   Development and Production data separate.
4. Generate a **Developer ID** distribution profile for
   `com.alkinum.removent.sync`, including CloudKit and production push notifications.
   Set `REMOVENT_CLOUDKIT_PROFILE` to its local path for `scripts/build/package.sh`.
   For CI, set `APPLE_CLOUDKIT_PROFILE_BASE64` to the encoded profile. Do not commit
   profiles, signing keys or account credentials.
5. Packaging embeds the profile in the helper and signs the actual CloudKit process.
   [`configure_cloud_sync.py`](../../scripts/build/configure_cloud_sync.py) validates its
   App ID, team, expiry, container, environment and notification capability.
   Release verification checks the embedded profile and actual signed entitlements.
   An unsigned development package shows a configuration status when enabled.
6. Export mobile distribution builds using the Production configuration/profile.
   Verify the exported entitlements; TestFlight/mobile and Developer ID desktop
   must both access the same Production container.

For local Mac development, the `apple` Xcode project creates the helper's signing
profile; the standalone helper is not a runnable desktop app because it expects
the storage CLI in its parent bundle. After building it with Xcode, package the
complete desktop app with an Apple Development identity and that helper profile:

```sh
APPLE_SIGNING_IDENTITY='Apple Development: …' \
REMOVENT_CLOUDKIT_ENVIRONMENT=Development \
REMOVENT_CLOUDKIT_PROFILE='/path/to/RemoventSync.app/Contents/embedded.provisionprofile' \
bash scripts/build/package.sh
```

Launch the resulting desktop bundle and an iOS Debug build under the same iCloud
account. Release packaging defaults to Production; release verification rejects
development profiles and signatures. The profile file should be outside `dist`,
which packaging regenerates.

## Validation and remaining acceptance

Tests cover encrypted record encoding, schema rejection, account callback isolation,
credential/trust isolation, offline edits, three-way conflicts, deletion, in-flight
acknowledgements, sign-out/account switching and journal crash recovery. UI tests
exercise the mobile settings toggle/configuration status and local opt-out. See
[`cloudkit-sync-validation.md`](../validation/cloudkit-sync-validation.md) for actual run results.

Live acceptance is still required with correctly signed builds: create/rename/edit/
delete in both directions; offline concurrent edits; quota/network failures;
notifications; account switching; schema deployment; and Mac/iPhone convergence.
Password synchronization via iCloud Keychain and
cross-platform preference sync remain follow-ups, as scoped in the original plan.

Apple references: [container setup](https://developer.apple.com/documentation/cloudkit/enabling-cloudkit-in-your-app),
[Developer ID capabilities](https://developer.apple.com/developer-id/),
[CKSyncEngine](https://developer.apple.com/documentation/cloudkit/cksyncengine-5sie5),
[encrypted record fields](https://developer.apple.com/documentation/cloudkit/ckrecord/encryptedvalues),
[iCloud Keychain](https://developer.apple.com/documentation/security/ksecattrsynchronizable),
[macOS distribution signing](https://developer.apple.com/documentation/xcode/creating-distribution-signed-code-for-the-mac/).
