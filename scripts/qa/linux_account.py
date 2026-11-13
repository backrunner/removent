#!/usr/bin/env python3
"""Exercise PAM and the QUIC account handshake with a disposable Linux user."""
import json
import os
from pathlib import Path
import secrets
import shutil
import subprocess
import tempfile


def main():
    if os.environ.get("GITHUB_ACTIONS") != "true":
        raise RuntimeError("Requires an ephemeral GitHub Actions runner")
    # Match the feature union of the preceding library regression step.
    command = ["cargo", "test", "--locked"]
    for package in ["proto", "core", "input", "client", "host", "media-codec"]:
        command.extend(["-p", f"removent-{package}"])
    build = subprocess.run(command + ["--lib", "--no-run", "--message-format=json"],
                           stdout=subprocess.PIPE, text=True, check=True, timeout=300)
    artifacts = [json.loads(line) for line in build.stdout.splitlines()]
    binaries = [item["executable"] for item in artifacts
                if item.get("reason") == "compiler-artifact"
                and item.get("profile", {}).get("test") and item.get("executable")
                and item.get("target", {}).get("name") == "removent_host"]
    if len(binaries) != 1:
        raise RuntimeError("Expected exactly one host test executable")
    username = "rmv" + secrets.token_hex(6)
    credentials = {"REMOVENT_AUTH_TEST_USERNAME": username,
                   "REMOVENT_AUTH_TEST_PASSWORD": secrets.token_hex(24) + "aA1!"}
    # Supply secrets through stdin, never shell arguments or a temporary file.
    child = """import json, os, sys
env = dict(os.environ, **json.load(sys.stdin))
os.execve(sys.argv[1], [sys.argv[1], "native_desktop_account_authentication", "--ignored"], env)
"""
    with tempfile.TemporaryDirectory(prefix="removent-account-") as directory:
        root = Path(directory)
        root.chmod(0o755)
        binary = root / "host-test"
        shutil.copyfile(binaries[0], binary)
        binary.chmod(0o755)
        created = False
        try:
            subprocess.run(["sudo", "useradd", "--create-home", "--shell", "/bin/sh", username],
                           check=True, timeout=30)
            created = True
            subprocess.run(["sudo", "chpasswd"],
                           input=f"{username}:{credentials['REMOVENT_AUTH_TEST_PASSWORD']}\n",
                           text=True, check=True, timeout=30)
            subprocess.run([str(binary), "native_desktop_account_authentication", "--ignored"],
                           env=dict(os.environ, **credentials, REMOVENT_AUTH_EXPECT_REJECT="1"),
                           cwd=directory, check=True, timeout=30)
            run_as_owner = ["sudo", "-u", username, "python3", "-c", child, str(binary)]
            subprocess.run(run_as_owner, input=json.dumps(credentials), text=True,
                           cwd=directory, check=True, timeout=30)
            subprocess.run(["sudo", "chage", "--expiredate", "0", username],
                           check=True, timeout=30)
            subprocess.run(run_as_owner,
                           input=json.dumps(dict(credentials, REMOVENT_AUTH_EXPECT_REJECT="1")),
                           text=True, cwd=directory, check=True, timeout=30)
            print("PASS: Linux PAM/QUIC password, invalid password, owner UID and expired account checks")
        finally:
            if created:
                subprocess.run(["sudo", "userdel", "--remove", username], check=True, timeout=30)


if __name__ == "__main__":
    main()
