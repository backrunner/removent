#!/usr/bin/env python3
"""Opt-in macOS GUI-session acceptance; private data/label, no capture or TCC prompts.

Build Rust app/daemon/CLI and run scripts/build_tray.sh first. Uses real packaged
tray resources and real daemon/launchd, including process death and manual stop.
"""
import argparse
import hashlib
import json
import fcntl
import os
from pathlib import Path
import shutil
import signal
import socket
import subprocess
import tempfile
import time


def wait_for(check, message, timeout=30):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = check()
        if value:
            return value
        time.sleep(0.2)
    raise AssertionError(message)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--live', action='store_true', required=True)
    parser.add_argument('--bin-dir', type=Path, default=Path('target/debug'))
    parser.add_argument('--tray', type=Path, default=Path('dist/RemoventTray.app'))
    args = parser.parse_args()
    binaries = args.bin_dir.resolve()
    children = []
    with tempfile.TemporaryDirectory(prefix='rv-bg-', dir='/tmp') as tmp:
        root = Path(tmp).resolve()
        data = root / 'data'
        data.mkdir()
        (data / 'settings.toml').write_text('host_enabled = false\n')
        bundle = root / 'Removent.app'
        macos = bundle / 'Contents/MacOS'
        macos.mkdir(parents=True)
        for name in ('removent-cli', 'removentd'):
            shutil.copy2(binaries / name, macos / name)
        tray = bundle / 'Contents/Helpers/RemoventTray.app'
        shutil.copytree(args.tray, tray)
        tray_bin = tray / 'Contents/MacOS/RemoventTray'
        env = dict(os.environ, REMOVENT_DATA_DIR=str(data), REMOVENT_TRAY_NO_ALERTS='1')
        env.pop('REMOVENT_DEV_SUPERVISED', None)
        label = 'com.alkinum.removent.daemon.' + hashlib.sha256(os.fsencode(data)).hexdigest()[:12]
        target = f'gui/{os.getuid()}/{label}'

        def cli(action):
            return json.loads(subprocess.check_output(
                [macos / 'removent-cli', 'daemon', action], env=env, timeout=25))

        def pid():
            result = subprocess.run(['/bin/launchctl', 'print', target], capture_output=True, text=True)
            for line in result.stdout.splitlines():
                if line.strip().startswith('pid = '):
                    return int(line.split('=')[1])
            return None

        log = (root / 'tray.log').open('w+')

        def start_tray():
            p = subprocess.Popen([tray_bin], env=env, stdout=log, stderr=log)
            children.append(p)
            return p

        try:
            first = start_tray()
            wait_for(lambda: cli('service-status')['reachable'], 'Independent tray did not start daemon')
            assert first.poll() is None, 'Packaged tray crashed'
            assert 'menu bar UI ready' in (root / 'tray.log').read_text()
            original = pid()
            assert original
            # Kernel locks cover simultaneous launches, not process-name guesses.
            duplicates = [start_tray() for _ in range(4)]
            assert all(p.wait(timeout=5) == 0 for p in duplicates)
            duplicate_daemon = subprocess.run([macos / 'removentd', '--background'],
                                            env=env, capture_output=True, timeout=5)
            assert duplicate_daemon.returncode != 0 and pid() == original
            print('PASS: packaged tray starts independently; tray and daemon remain singletons', flush=True)

            os.kill(original, signal.SIGKILL)
            recovered = wait_for(lambda: pid() if pid() != original and cli('service-status')['reachable'] else None,
                                 'Crash recovery failed')
            assert recovered != original
            # Remove launchd ownership altogether: recovery now requires tray.
            subprocess.run(['/bin/launchctl', 'bootout', target], check=True, capture_output=True)
            wait_for(lambda: pid() if pid() != recovered and cli('service-status')['reachable'] else None,
                     'Tray did not recreate the missing job')
            print('PASS: crash and missing launchd job both recover', flush=True)

            assert cli('login-on')['launch_at_login']
            assert cli('stop')['stopped_by_user']
            # Simulate login loading the persisted job. RunAtLoad may invoke the
            # binary once, but explicit stop must suppress hosting and KeepAlive.
            login_file = Path.home() / f'Library/LaunchAgents/{label}.plist'
            subprocess.run(['/bin/launchctl', 'bootstrap', f'gui/{os.getuid()}', login_file],
                           check=True, capture_output=True)
            first.terminate()
            first.wait(timeout=5)
            first = start_tray()
            time.sleep(7)
            assert not cli('service-status')['reachable'] and pid() is None
            assert first.poll() is None
            assert not cli('ensure')['reachable']
            print('PASS: explicit stop survives tray relaunch, login job loading and watchdog polls', flush=True)

            assert cli('start')['reachable']
            with socket.socket(socket.AF_UNIX) as client:
                client.settimeout(5)
                client.connect(str(data / 'run/removentd.sock'))
                client.sendall(b'{"type":"shutdown"}\n')
                client.recv(4096)
            wait_for(lambda: pid() is None and cli('service-status')['stopped_by_user'],
                     'IPC shutdown was not retained')
            time.sleep(3)
            assert not cli('service-status')['reachable']
            assert cli('start')['reachable']
            before_login = pid()
            assert cli('login-on')['launch_at_login']
            assert not cli('login-off')['launch_at_login']
            assert pid() == before_login
            first.terminate()
            first.wait(timeout=5)
            assert cli('service-status')['reachable']
            os.kill(before_login, signal.SIGKILL)
            wait_for(lambda: pid() if pid() != before_login and cli('service-status')['reachable'] else None,
                     'launchd did not recover service with both UIs closed')
            print('PASS: login toggles preserve process; service survives tray exit and recovers without UI', flush=True)

            # The desktop must launch the embedded tray with the same data root.
            # Test from a relocated .app so source-tree fallbacks cannot help.
            assert cli('stop')['stopped_by_user']
            shutil.copy2(binaries / 'removent', macos / 'removent')
            import plistlib
            (bundle / 'Contents/Info.plist').write_bytes(plistlib.dumps({
                'CFBundleExecutable': 'removent', 'CFBundleName': 'Removent Test',
                'CFBundleIdentifier': 'com.alkinum.removent.lifecycle-test',
                'CFBundlePackageType': 'APPL', 'NSHighResolutionCapable': True,
            }))
            app_env = dict(env, REMOVENT_NO_UPDATE_CHECK='1')
            app_env.pop('REMOVENT_NO_TRAY', None)
            app = subprocess.Popen([macos / 'removent'], env=app_env, stdout=log, stderr=log)
            children.append(app)

            def tray_pid():
                output = subprocess.check_output(['ps', '-axo', 'pid,command'], text=True)
                for line in output.splitlines():
                    parts = line.strip().split(None, 1)
                    if len(parts) == 2 and parts[1] == str(tray_bin):
                        return int(parts[0])
                return None

            launched = wait_for(tray_pid, 'Desktop did not launch its embedded tray')
            try:
                with (data / '.tray.lock').open('r') as lock:
                    try:
                        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    except BlockingIOError:
                        pass
                    else:
                        raise AssertionError('Desktop tray used the wrong data directory')
                assert app.poll() is None
                assert not cli('service-status')['reachable']
                app.terminate()
                app.wait(timeout=5)
                assert tray_pid() == launched, 'Tray exited with desktop'
                print('PASS: desktop launches relocated embedded tray; tray survives desktop exit; manual stop retained', flush=True)
            finally:
                try:
                    os.kill(launched, signal.SIGTERM)
                except ProcessLookupError:
                    pass
        finally:
            for child in children:
                if child.poll() is None:
                    child.terminate()
                child.wait(timeout=5)
            cli('login-off')
            cli('stop')
            log.close()


if __name__ == '__main__':
    main()
