# Automatic update acceptance, 2026-11-07

Scope: user-session macOS updates, independent recovery, permission preflight and startup safety. All runtime fixtures use separate data directories, application copies and launchd labels. The installed application, administrator-managed login-window host and br-gateway service are preserved.

The local signed package starts its real GPUI desktop, renders the first frame, remains alive for ten seconds and exits cleanly. Its real daemon answers versioned IPC for ten seconds with sharing disabled and exits cleanly on SIGTERM. The independent updater executable passes its startup self-test. This check is also a development-package CI gate and runs before release notarization.

```sh
python3 scripts/qa/test_update_startup.py --bundle dist/Removent.app
```

The opt-in transaction tests use Developer ID signed local fault fixtures. A candidate adding a permission declaration is rejected without changing the old host PID. A candidate whose host immediately exits rolls back to the prior signed application, preserves device identity and restarts the old host. A higher repair candidate uses the real packaged desktop and daemon, passes 60 seconds of continuous health observation, and retains its rollback copy. A separate test updates an explicitly stopped host without starting the real user service. A journal-write failure test checks that restoration still resumes the verified prior host.

```sh
REMOVENT_UPDATE_TEST_SIGNER="$APPLE_SIGNING_IDENTITY" \
REMOVENT_UPDATE_TEST_BUNDLE="$PWD/dist/Removent.app" \
cargo test -p removent-updater agent::runtime_tests:: -- --ignored --nocapture --test-threads=1
```

The independent supervisor acceptance uses actual launchd processes. It kills an installer after the original application's rename, leaving the application path absent. The pinned recovery channel restores the signed backup and restarts the host with the same identity. A separately signed updater passes its self-test and then crashes; the supervisor rejects it and resumes the known working engine. A subsequent working signed worker is promoted. Killing the supervisor itself does not orphan its worker or erase a manual host stop.

```sh
python3 scripts/qa/test_update_recovery.py --live \
  --bundle dist/Removent.app --signer "$APPLE_SIGNING_IDENTITY"
```

Unit regressions cover interrupted rename boundaries, repeated rollback, FIFO/symlink/state rejection, damaged settings, default-on migration with existing check opt-out, persistent maintenance admission, shared/exclusive gate races, wrong version/identity/permission reports, PID changes during probation, failed-version quarantine and command deadlines. The legacy signed-manifest, channel and download regressions remain in the shared updater package.

Limits: the crash fixtures deliberately use fake failing executables, and no bad release is published. The GUI and successful repair host are the real package. These tests do not revoke production TCC grants, modify administrator installation ownership, fill a physical disk, simulate power failure of an APFS device, or prove all future updates and runtime features correct. Local evidence is separate from public artifact notarization, exact-commit CI, and remote-device acceptance.
