#!/usr/bin/env python3
"""Use a fresh Linux Secret Service collection or Windows Credential Manager."""
import argparse
import json
import os
from pathlib import Path
import secrets
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--bin-dir", type=Path, default=Path("target/release"))
    parser.add_argument("--app-tests", action="store_true",
                        help="also exercise the desktop remember, reuse and edit flows")
    args = parser.parse_args()
    binary = args.bin_dir.resolve() / "examples" / ("credential_smoke.exe" if os.name == "nt" else "credential_smoke")
    with tempfile.TemporaryDirectory(prefix="removent-secrets-") as data:
        env = dict(os.environ, REMOVENT_DESKTOP_TEST="1", XDG_DATA_HOME=data)
        if os.name != "nt":
            unavailable = dict(env, DBUS_SESSION_BUS_ADDRESS=f"unix:path={data}/absent-bus")
            subprocess.run([str(binary), "--expect-unavailable"], env=unavailable,
                           check=True, timeout=20)
            if any(Path(data).rglob("*")):
                raise RuntimeError("An unavailable credential store wrote a fallback file")
            control = str(Path(data) / "control")
            env["GNOME_KEYRING_CONTROL"] = control
            subprocess.run(["gnome-keyring-daemon", "--unlock", "--control-directory", control],
                           input=secrets.token_hex(32).encode(), env=env, check=True, timeout=15)
            subprocess.run(["gnome-keyring-daemon", "--start", "--components=secrets", "--control-directory", control],
                           env=env, check=True, timeout=15)
        subprocess.run([str(binary)], env=env, check=True, timeout=20)
        if args.app_tests:
            build = subprocess.run(
                ["cargo", "test", "--locked", "-p", "removent-app", "--bin", "removent",
                 "--no-run", "--message-format=json"],
                stdout=subprocess.PIPE, text=True, check=True, timeout=1200)
            artifacts = [json.loads(line) for line in build.stdout.splitlines()]
            binaries = [item["executable"] for item in artifacts
                        if item.get("reason") == "compiler-artifact"
                        and item.get("profile", {}).get("test") and item.get("executable")
                        and item.get("target", {}).get("name") == "removent"]
            if len(binaries) != 1:
                raise RuntimeError("Expected exactly one desktop test executable")
            for test_filter, extra in [
                ("engine::credentials::tests::", []),
                ("ui::home::tests::authentication::", []),
                ("ui::connection::", []),
                ("engine::credentials::tests::", ["--ignored"]),
            ]:
                subprocess.run([binaries[0], test_filter, "--test-threads=1", *extra],
                               env=env, check=True, timeout=120)


if __name__ == "__main__":
    main()
