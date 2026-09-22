#!/usr/bin/env python3
"""Opt-in real launchd acceptance using an isolated label, port and config."""
import argparse
import hashlib
import json
import os
import pathlib
import signal
import socket
import subprocess
import sys
import tarfile
import tempfile
import time


def run(binary):
    assert sys.platform == "darwin" and os.geteuid() != 0
    domain = f"gui/{os.geteuid()}"
    subprocess.run(["/bin/launchctl", "print", domain], check=True, stdout=subprocess.DEVNULL)
    with tempfile.TemporaryDirectory(prefix="removent relay launchd ") as directory:
        root = pathlib.Path(directory).resolve()
        config = root / "relay"
        scripts = pathlib.Path(__file__).resolve().parent
        version = "v" + subprocess.check_output([str(binary), "--version"], text=True).strip().split()[1]
        name = f"removent-relay-{version}-macos-universal.tar.gz"
        # Offline transport fixture containing the actual tested executable.
        # Release CI supplies the signed universal binary to this same test.
        with tarfile.open(root / name, "w:gz") as archive:
            archive.add(binary, arcname="removent-relay")
        (root / (name + ".sha256")).write_text(f"{hashlib.sha256((root / name).read_bytes()).hexdigest()}  {name}\n")
        mock = root / "download"
        mock.mkdir()
        curl = mock / "curl"
        curl.write_text(f'''#!{sys.executable}
import pathlib, shutil, sys
args = sys.argv[1:]
assert args[-1].startswith("https://github.com/backrunner/removent/releases/download/{version}/")
source = pathlib.Path({str(root)!r}) / args[-1].rsplit("/", 1)[1]
shutil.copyfile(source, args[args.index("--output") + 1])
''')
        curl.chmod(0o755)
        binary = root / "bin/removent-relay"
        binary.parent.mkdir()

        def install(*setup_args):
            args = ["sh", str(scripts / "install_relay.sh"), "--version", version,
                    "--bin-dir", str(binary.parent), "--service-dir", str(config)]
            if setup_args:
                args.extend(["--", *setup_args])
            result = subprocess.run(args, env=dict(os.environ, PATH=f"{mock}:{os.environ['PATH']}"),
                                    stdin=subprocess.DEVNULL, text=True, capture_output=True, timeout=35)
            assert result.returncode == 0, result.stdout + result.stderr
        label = "com.alkinum.removent.relay." + hashlib.sha256(os.fsencode(config)).hexdigest()[:12]
        login = pathlib.Path.home() / "Library/LaunchAgents" / f"{label}.plist"
        assert not login.exists() and not login.is_symlink()

        def cli(*args, ok=True):
            result = subprocess.run([str(binary), "--service-dir", str(config), *args], text=True,
                                    capture_output=True, timeout=35)
            assert (result.returncode == 0) == ok, f"{args}: {result.stdout}\n{result.stderr}"
            return result.stdout

        def status():
            return json.loads(cli("status"))

        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        try:
            install("--address", f"removent://127.0.0.1:{port}", "--listen", f"127.0.0.1:{port}", "--no-start")
            assert not status()["running"] and not status()["launch_at_login"]
            saved = {p.name: p.read_bytes() for p in config.glob("*.toml")}
            pin = cli("fingerprint").strip()
            cli("check")
            cli("export", "client", "--output", str(pathlib.Path(directory) / "client.toml"), "--host-fingerprint", "ab" * 32)
            cli("start")
            original = status()
            assert original["running"] and not original["launch_at_login"]
            cli("start")
            assert status()["pid"] == original["pid"]
            cli("enable")
            assert status()["launch_at_login"] and login.is_file()
            assert login.stat().st_mode & 0o777 == 0o600
            cli("disable")
            assert not status()["launch_at_login"] and not login.exists()
            assert status()["pid"] == original["pid"]
            os.kill(original["pid"], signal.SIGKILL)
            deadline = time.monotonic() + 25
            while True:
                recovered = status()
                if recovered["running"] and recovered["pid"] != original["pid"]:
                    break
                assert time.monotonic() < deadline, "launchd did not recover the relay"
                time.sleep(0.2)
            cli("restart")
            assert status()["running"] and status()["pid"] != recovered["pid"]
            assert cli("fingerprint").strip() == pin
            # A bad edit must not tear down an otherwise working relay.
            server = config / "server.toml"
            server.write_text("invalid configuration")
            pid = status()["pid"]
            cli("restart", ok=False)
            assert status()["pid"] == pid and status()["running"]
            server.write_bytes(saved["server.toml"])
            assert "Relay listening" in cli("logs", "--lines", "30")
            cli("stop")
            assert not status()["running"] and not status()["loaded"]
            assert not (config / "run/ready.pid").exists()
            install()
            assert not status()["running"] and not status()["launch_at_login"]
            assert saved == {p.name: p.read_bytes() for p in config.glob("*.toml")}
            # A occupied socket must not be reported as a successful start.
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as blocked:
                blocked.bind(("127.0.0.1", port))
                cli("start", ok=False)
            assert not status()["loaded"]
            cli("setup", "--no-start")
            assert saved == {p.name: p.read_bytes() for p in config.glob("*.toml")}
            assert not status()["running"] and not status()["launch_at_login"]
            cli("enable")
            cli("start")
            cli("uninstall")
            assert not login.exists()
            assert not (config / "service.json").exists()
            assert saved == {p.name: p.read_bytes() for p in config.glob("*.toml")}
            assert cli("fingerprint").strip() == pin
            cli("setup", "--no-start")
            cli("start")
            assert status()["running"]
            cli("uninstall")
            print("Real macOS launchd lifecycle, login startup, crash recovery, readiness, logs, export and identity retention passed")
        finally:
            subprocess.run(["/bin/launchctl", "bootout", f"{domain}/{label}"], stdout=subprocess.DEVNULL,
                           stderr=subprocess.DEVNULL, check=False)
            login.unlink(missing_ok=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--live", action="store_true", required=True)
    parser.add_argument("--binary", type=pathlib.Path, default=pathlib.Path("target/debug/removent-relay"))
    args = parser.parse_args()
    run(args.binary.resolve(strict=True))
