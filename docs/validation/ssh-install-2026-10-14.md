# SSH installer validation (2026-10-14)

The existing `scripts/install.sh` removed the old bundle before copying, parsed
release JSON with line-oriented text matching, and did not verify checksums,
component signatures, rollback or service state. It has been replaced with a
release-verified, staged installer using macOS system tools.

## Evidence

- `bash -n scripts/install.sh` and `shellcheck scripts/install.sh` passed.
- `python3 scripts/macos/test_install.py`: 16 isolated installer cases passed.
  Coverage includes stdin streaming, spaces in paths, fresh install, upgrade,
  manual stop, explicit start/login, checksum/download/signature/Gatekeeper/copy
  failures, replacement and service-refresh rollback, interruption at both move boundaries,
  retained backup after
  rollback failure, active UI/session refusal, another daemon using the bundle,
  unsupported OS/architecture, symlink/lock refusal and sudo-only file operations.
- `python3 -m unittest discover -s scripts/macos -p 'test_*.py'`: all 20 cases passed.
- `bash scripts/check.sh` passed the repository's offline checks: layout, build,
  release, relay and macOS suites, plus shell syntax. The relay suite has one
  expected skip.
- `bash scripts/install.sh --verify-only` downloaded real stable `v0.1.4` and
  passed SHA-256, bundle version, six component identities/signatures and
  Gatekeeper assessment. The DMG was detached after verification.
- `python3 scripts/qa/test_ssh_install.py --live` passed against the published
  `v0.1.4` bundle over a real temporary key-only loopback SSH connection. It
  installed to a private application directory, started a real private launchd
  daemon, registered login startup, reinstalled and observed a changed daemon PID,
  then stopped and reinstalled again. IPC reachability, preserved login preference,
  preserved disabled sharing, persistent manual stop and staging cleanup all passed.
  The private service, login registration, SSH server, keys, bundle and data were
  removed afterwards. The normal `/Applications/Removent.app` was not replaced.

Verified DMG: `Removent-0.1.4-macos-arm64.dmg`, SHA-256
`98587bb3f1ea9f43b0836eb07964fb922dcf9f2d9680f6170518bcf4053f0d7d`.

## Limits

The live test used the existing Mac's graphical login and disabled hosting. It
validates SSH installation and launchd replacement, not separate-host/WAN access,
first-time TCC approval, capture, input, FileVault or login-window hosting. The
sudo branch was exercised with isolated command fixtures, not by replacing the
system app under administrator privileges. Current scripts are workspace changes;
these results do not establish that GitHub `main` or remote CI contains them.

The installer retains the existing requirement for macOS 26+, Apple silicon and
an active graphical login when starting/refreshing the user-session service.
First-time screen/input privacy approval must be completed on the target Mac.
