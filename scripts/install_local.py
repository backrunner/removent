#!/usr/bin/env python3
"""Build and install this checkout's macOS app, daemon and tray together."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import shutil
import signal
import socket
import subprocess
import sys
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts/build'))
from local_signing import configure_local_signing


def run(*args, **kwargs):
    return subprocess.run(args, check=True, **kwargs)


def stop_ui(bundle):
    executables = {str(bundle / 'Contents/MacOS/removent'),
                   str(bundle / 'Contents/Helpers/RemoventTray.app/Contents/MacOS/RemoventTray')}
    output = subprocess.check_output(['ps', '-axo', 'pid=,comm='], text=True)
    pids = []
    for line in output.splitlines():
        parts = line.strip().split(None, 1)
        if len(parts) == 2 and parts[1] in executables:
            pid = int(parts[0])
            os.kill(pid, signal.SIGTERM)
            pids.append(pid)
    deadline = time.monotonic() + 10
    while pids and time.monotonic() < deadline:
        remaining = []
        for pid in pids:
            try:
                os.kill(pid, 0)
                remaining.append(pid)
            except ProcessLookupError:
                pass
        pids = remaining
        if pids:
            time.sleep(0.1)
    if pids:
        raise RuntimeError('Removent UI did not quit; installation has not replaced the app.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--no-build', action='store_true', help='Use a bundle only if its source fingerprint is current')
    parser.add_argument('--destination', type=Path, default=Path('/Applications/Removent.app'))
    args = parser.parse_args()
    destination = args.destination.expanduser().absolute()
    source = ROOT / 'dist/Removent.app'
    if source == destination or destination.suffix != '.app':
        parser.error('Choose an installed .app path outside dist/Removent.app')
    if not args.no_build:
        build_env = configure_local_signing(os.environ, destination)
        run('bash', str(ROOT / 'scripts/build/package.sh'), cwd=ROOT, env=build_env)
    run('python3', str(ROOT / 'scripts/build/build_stamp.py'), 'verify', '--app', str(source))
    run('codesign', '--verify', '--deep', '--strict', str(source))
    data = Path(os.environ.get('REMOVENT_DATA_DIR',
                str(Path.home() / 'Library/Application Support/removent/userdata'))).expanduser().resolve()
    env = dict(os.environ, REMOVENT_DATA_DIR=str(data))
    cli = source / 'Contents/MacOS/removent-cli'

    def service(action, executable=cli):
        return json.loads(subprocess.check_output([executable, 'daemon', action], env=env, text=True))

    before = service('service-status')
    if before['reachable']:
        with socket.socket(socket.AF_UNIX) as connection:
            connection.settimeout(5)
            connection.connect(str(data / 'run/removentd.sock'))
            connection.sendall(b'{"type":"status"}\n')
            status = json.loads(connection.makefile('rb').readline())
            if status.get('sessions'):
                raise SystemExit('Finish active remote sessions before installing a new local build.')
    old_bundles = {source, destination}
    for plist in (data / 'run').glob('com.alkinum.removent.daemon*.plist'):
        arguments = plistlib.loads(plist.read_bytes()).get('ProgramArguments', [])
        if arguments:
            executable = Path(arguments[0])
            if executable.name == 'removentd' and executable.parent.name == 'MacOS':
                bundle = executable.parents[2]
                if bundle.name == 'RemoventHost.app':
                    bundle = bundle.parents[2]
                old_bundles.add(bundle)
    destination.parent.mkdir(parents=True, exist_ok=True)
    token = uuid.uuid4().hex
    staged = destination.parent / f'.Removent-stage-{token}.app'
    backup = destination.parent / f'.Removent-backup-{token}.app'
    run('ditto', str(source), str(staged))
    run('codesign', '--verify', '--deep', '--strict', str(staged))
    for bundle in old_bundles:
        stop_ui(bundle)
    service('stop')
    try:
        if destination.exists():
            destination.rename(backup)
        staged.rename(destination)
        installed_cli = destination / 'Contents/MacOS/removent-cli'
        if before['launch_at_login']:
            service('login-on', installed_cli)
        # Keep an existing tray login preference, updating its exact bundle path.
        tray_login = Path.home() / 'Library/LaunchAgents/com.alkinum.removent.tray.plist'
        if tray_login.exists():
            info = plistlib.loads(tray_login.read_bytes())
            if info.get('Label') == 'com.alkinum.removent.tray':
                info['ProgramArguments'] = ['/usr/bin/open', '-g', '-n', '--env',
                    f'REMOVENT_DATA_DIR={data}', str(destination / 'Contents/Helpers/RemoventTray.app')]
                temporary = tray_login.with_suffix('.tmp')
                temporary.write_bytes(plistlib.dumps(info))
                temporary.replace(tray_login)
        if not before['stopped_by_user']:
            service('start', installed_cli)
        for relative in ['Contents/MacOS/removent', 'Contents/MacOS/removentd',
                         'Contents/Helpers/RemoventTray.app/Contents/MacOS/RemoventTray']:
            expected = hashlib.sha256((source / relative).read_bytes()).hexdigest()
            actual = hashlib.sha256((destination / relative).read_bytes()).hexdigest()
            if expected != actual:
                raise RuntimeError(f'Installed binary differs from build: {relative}')
            print(f'Verified {relative}: {actual}')
        run('/usr/bin/open', '--env', f'REMOVENT_DATA_DIR={data}', str(destination))
    except Exception:
        print(f'Installation failed. Previous app retained at {backup}; service state needs checking.')
        raise
    finally:
        if staged.exists():
            shutil.rmtree(staged)
    if backup.exists():
        shutil.rmtree(backup)
    print(f'Installed current checkout at {destination}')


if __name__ == '__main__':
    main()
