#!/usr/bin/env python3
"""Opt-in, real systemd acceptance on a disposable Linux CI runner only."""
import hashlib
import os
import pathlib
import shutil
import socket
import subprocess
import sys
import tempfile

BINARY = "/usr/local/bin/removent-relay"
CONFIG = pathlib.Path("/etc/removent-relay")
STATE = pathlib.Path("/var/lib/removent-relay")
PRIVATE_STATE = pathlib.Path("/var/lib/private/removent-relay")
UNIT = pathlib.Path("/etc/systemd/system/removent-relay.service")


def cli(*args, ok=True):
    result = subprocess.run([BINARY, *args], capture_output=True, text=True, timeout=45)
    if ok and result.returncode:
        raise AssertionError(f"{args}: {result.stdout}\n{result.stderr}")
    if not ok and result.returncode == 0:
        raise AssertionError(f"Expected failure: {args}")
    return result.stdout


def reinstall_from_release_fixture():
    """Run the real installer and real packaged binary, with offline downloads."""
    scripts = pathlib.Path(__file__).resolve().parent
    version = "v" + cli("--version").strip().split()[1]
    platform = "linux-aarch64" if os.uname().machine == "aarch64" else "linux-x86_64"
    with tempfile.TemporaryDirectory(prefix="relay-upgrade-") as directory:
        root = pathlib.Path(directory)
        subprocess.run([sys.executable, str(scripts / "package_relay.py"), "--version", version,
                        "--platform", platform, "--binary", BINARY, "--directory", directory], check=True)
        mock_curl = root / "curl"
        mock_curl.write_text(f'''#!{sys.executable}
import pathlib, shutil, sys
args = sys.argv[1:]
url = args[-1]
assert url.startswith("https://github.com/backrunner/removent/releases/download/{version}/")
source = pathlib.Path({directory!r}) / url.rsplit("/", 1)[1]
shutil.copyfile(source, args[args.index("--output") + 1])
''')
        mock_curl.chmod(0o755)
        environment = dict(os.environ, PATH=f"{root}:{os.environ['PATH']}")
        subprocess.run(["sh", str(scripts / "install_relay.sh"), "--version", version],
                       env=environment, stdin=subprocess.DEVNULL, check=True, timeout=30)


def run():
    assert sys.platform == "linux" and os.geteuid() == 0
    assert pathlib.Path("/run/systemd/system").is_dir()
    # Never interfere with an existing installation, even when --live is passed.
    for path in (CONFIG, STATE, PRIVATE_STATE, UNIT):
        assert not path.exists() and not path.is_symlink(), f"Existing installation: {path}"
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    try:
        cli("setup", "--address", f"removent://127.0.0.1:{port}", "--listen", f"127.0.0.1:{port}", "--no-start")
        assert "ActiveState=inactive" in cli("status")
        saved = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in CONFIG.glob("*.toml")}
        assert CONFIG.stat().st_mode & 0o777 == 0o700
        for path in CONFIG.glob("*.toml"):
            assert path.stat().st_mode & 0o777 == 0o600 and path.stat().st_uid == 0
        fingerprint = cli("fingerprint").strip()
        assert len(fingerprint) == 64
        cli("start")
        assert "ActiveState=active" in cli("status")
        process_id = int(subprocess.check_output(["systemctl", "show", "--property=MainPID", "--value", "removent-relay.service"], text=True))
        assert pathlib.Path(f"/proc/{process_id}").stat().st_uid != 0
        assert "UnitFileState=disabled" in cli("status")
        cli("enable")
        assert "UnitFileState=enabled" in cli("status")
        cli("restart")
        assert cli("fingerprint").strip() == fingerprint
        cli("logs", "--lines", "10")
        cli("stop")
        assert "ActiveState=inactive" in cli("status")
        cli("disable")
        assert "UnitFileState=disabled" in cli("status")
        reinstall_from_release_fixture()
        assert "ActiveState=inactive" in cli("status")
        assert "UnitFileState=disabled" in cli("status")
        cli("setup", "--no-start")
        cli("setup", "--address", "removent://different.example:48700", ok=False)
        assert saved == {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in CONFIG.glob("*.toml")}
        # An invalid edit must fail before disrupting or starting the service.
        config = CONFIG / "server.toml"
        original = config.read_bytes()
        config.write_text("invalid config")
        cli("start", ok=False)
        assert "ActiveState=inactive" in cli("status")
        config.write_bytes(original)
        cli("start")
        cli("uninstall")
        assert not UNIT.exists()
        assert CONFIG.is_dir() and STATE.is_dir()
        assert cli("fingerprint").strip() == fingerprint
        assert saved == {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in CONFIG.glob("*.toml")}
        print("Native systemd setup, readiness, lifecycle, permissions and identity retention passed")
    except Exception:
        subprocess.run(["systemctl", "status", "--no-pager", "removent-relay.service"], check=False)
        subprocess.run(["journalctl", "--unit", "removent-relay.service", "--lines", "50", "--no-pager"], check=False)
        raise
    finally:
        if UNIT.exists():
            subprocess.run(["systemctl", "disable", "--now", "removent-relay.service"], check=False)
            UNIT.unlink()
            subprocess.run(["systemctl", "daemon-reload"], check=False)
        shutil.rmtree(CONFIG, ignore_errors=True)
        if STATE.is_symlink():
            STATE.unlink()
        else:
            shutil.rmtree(STATE, ignore_errors=True)
        shutil.rmtree(PRIVATE_STATE, ignore_errors=True)


if __name__ == "__main__":
    if sys.argv[1:] != ["--live"]:
        sys.exit("Requires --live on a disposable Linux runner; never run on a deployed relay")
    run()
