# Install Removent over SSH

Use `scripts/install.sh` on an Apple silicon Mac running macOS 26 or later.
No Rust, Xcode, Python or graphical installer is required. Log in over SSH as the
Mac user who will host sessions; do not run the whole script with sudo.
The Linux/macOS private relay has its own `scripts/install_relay.sh` installer.

## Install from this checkout

From the repository on your development machine, stream the Bash script to SSH:

```bash
ssh user@mac 'bash -s -- --app-dir "$HOME/Applications" --login' < scripts/install.sh
```

This installs `~/Applications/Removent.app` without administrator access, starts
the background daemon and enables its future user-login startup. It follows the
saved sharing switch (enabled by default on a new host). It does not open the
desktop or tray, change authentication, or grant privacy access.
Login startup is enabled by default on a new installation. `--login` explicitly enables it again;
`--start` starts without changing login preferences. `--no-login` installs without starting
and disables login startup.
Run `--help` for the full options. `REMOVENT_DATA_DIR`, when set on the remote
machine, must match the directory used by that host's desktop and daemon.

For `/Applications`, transfer the script to a user-owned remote directory, then
run `ssh -t user@mac 'bash /path/to/install.sh --login'`. Only file operations
use sudo when necessary; service control always runs as the SSH user. Streaming
without a terminal works if the destination is writable or sudo is already
pre-authorized. Otherwise the script fails with instructions before replacement.

Once the updated script is published on `main`, the remote Mac can download it:

```bash
curl -fsSL https://raw.githubusercontent.com/backrunner/removent/main/scripts/install.sh | bash -s -- --login
```

The script resolves the latest stable release. To select a published beta or pin
a stable version, pass `--version v0.1.4` (or set `REMOVENT_VERSION`). It validates
the requested tag and requires the matching DMG and `SHA256SUMS`; a release missing
either is rejected. `REMOVENT_REPO` can select a fork, but all shipped executables
must still carry the official Removent Developer ID identity.

## Upgrade and failure handling

Quit the installed desktop and menu bar app and finish remote sessions first.
Run the same command to reinstall or upgrade. The installer checks active processes
and daemon sessions and refuses replacement while any are still using the bundle.
A service for another user or data directory must also be stopped before replacement.

Before modification, it verifies the SHA-256 checksum, full release version,
expected bundle identifier, every shipped executable's Apple Developer ID signature
and Removent team, and Gatekeeper acceptance. It stages and re-verifies a complete
bundle on the destination filesystem, retains the old bundle while swapping, and
resumes a previously running service with the new executable. Old files are not
merged into the new bundle. Existing data, credentials, sharing settings and login
preferences are retained; an explicitly stopped service stays stopped unless
`--start` or `--login` was requested. The tray login preference is untouched.

Copy, replacement, installed-signature or service-refresh failure restores the
previous app and attempts to resume its prior service. If rollback itself fails,
the script retains the backup and reports its path for manual recovery. An explicit
first start or login setup is also part of the installation transaction. A failure
attempts rollback before reporting the startup error.

A destination lock prevents concurrent installers. After an uncatchable process
termination, inspect `.removent-install.lock` and any `.removent-install.*` backups
in the chosen application directory before removing a stale lock. INT, TERM and
normal failures run cleanup, including disk-image detach. A detach failure reports
the mount for manual ejection and retains its temporary directory.

## Hosting and permissions

Use `--no-login` to install with no graphical login. Starting or refreshing the
user-session service requires the SSH user to have a graphical `gui/<uid>` session.
`--login` refers to future **user login**, not pre-login system boot. A user-session
daemon does not unlock FileVault or provide a logged-out desktop.

New builds register both `removent` and `removent-cli` automatically. If `/usr/local/bin` is not writable, setup uses `~/.local/bin` and configures zsh, bash and fish for the installing user. Open a new terminal or login session to load the PATH change. Existing unrelated commands are preserved; a setup failure is reported after keeping the verified app installed. Releases predating the bundled `install-cli.sh` helper still use the full path below. For scripts with a restricted PATH, the full path always works (change it for `/Applications`):

```bash
REMOVENT_CLI="$HOME/Applications/Removent.app/Contents/MacOS/removent-cli"
"$REMOVENT_CLI" daemon service-status # launchd, login preference and IPC reachability
"$REMOVENT_CLI" daemon status         # sharing, actual privacy grants and host readiness
"$REMOVENT_CLI" daemon enable         # enable sharing explicitly
"$REMOVENT_CLI" daemon login-on       # start now and persist future user logins
"$REMOVENT_CLI" daemon stop           # explicit persistent stop
"$REMOVENT_CLI" pairing generate      # authorize a connection by code after host setup
```

First-time Screen Recording and Accessibility approval must be completed in the
Mac's System Settings for the actual daemon. Restart it after granting access.
A reachable management socket is not proof that screen capture or input is ready;
check `daemon status`, including permissions, `host_ready` and `host_error`.
See [background service setup](macos-background-service.md) for the full process.

## Validation

```bash
bash -n scripts/install.sh
shellcheck scripts/install.sh
python3 scripts/macos/test_install.py
bash scripts/install.sh --verify-only --version v0.1.4
python3 scripts/qa/test_ssh_install.py --live --version v0.1.4
```

The offline suite exercises isolated release and macOS fixtures, including piped
stdin, spaces in paths, download/checksum/signature failures, active processes and
sessions, rollback, service restoration and manual-stop preservation. It is included
in `scripts/check.sh` and CI through macOS test discovery.

`--verify-only` validates a real release and detaches its DMG without installing.
The opt-in live test uses the real published bundle, an ephemeral key-only loopback
SSH server and private installation/data/launchd paths with sharing disabled. It
checks installation/start, daemon PID replacement on upgrade, login/sharing preference
preservation and persistent manual stop, then removes its private service and login
registration. It requires an active graphical session and does not validate WAN SSH,
first-time privacy grants, remote capture or control on a separate host.
