#!/usr/bin/env python3
"""Use a fresh Linux Secret Service collection or Windows Credential Manager."""
import argparse
import os
from pathlib import Path
import secrets
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--bin-dir", type=Path, default=Path("target/release"))
    args = parser.parse_args()
    binary = args.bin_dir.resolve() / "examples" / ("credential_smoke.exe" if os.name == "nt" else "credential_smoke")
    with tempfile.TemporaryDirectory(prefix="removent-secrets-") as data:
        env = dict(os.environ, REMOVENT_DESKTOP_TEST="1", XDG_DATA_HOME=data)
        if os.name != "nt":
            control = str(Path(data) / "control")
            env["GNOME_KEYRING_CONTROL"] = control
            subprocess.run(["gnome-keyring-daemon", "--unlock", "--control-directory", control],
                           input=secrets.token_hex(32).encode(), env=env, check=True, timeout=15)
            subprocess.run(["gnome-keyring-daemon", "--start", "--components=secrets", "--control-directory", control],
                           env=env, check=True, timeout=15)
        subprocess.run([str(binary)], env=env, check=True, timeout=20)


if __name__ == "__main__":
    main()
