#!/usr/bin/env python3
"""Opt-in signed recovery acceptance using isolated launchd jobs and fault fixtures.

Never changes /Applications, the LoginWindow host, TCC or real user data.
Requires a Developer ID identity; bad workers exist only in the temporary fixture.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import signal
import subprocess
import tempfile
import time
from test_update_startup import health


def wait_for(check, message, timeout=45):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            value = check()
            if value:
                return value
        except (OSError, ValueError, KeyError):
            pass
        time.sleep(0.2)
    raise AssertionError(message)


def read(path, default=None):
    try:
        return json.loads(path.read_text())
    except FileNotFoundError:
        return default


def write(path, value):
    with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as output:
        output.write(json.dumps(value).encode())
        output.flush()
        os.fsync(output.fileno())
        staged = output.name
    os.replace(staged, path)


def run(*args, **kwargs):
    return subprocess.run(args, check=True, capture_output=True, timeout=40, **kwargs)


def job_pid(target):
    output = subprocess.run(['/bin/launchctl', 'print', target], capture_output=True, text=True)
    match = re.search(r'^\s*pid = (\d+)$', output.stdout, re.MULTILINE)
    return int(match[1]) if match else None


def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--live', action='store_true', required=True)
    parser.add_argument('--bundle', type=Path, default=Path('dist/Removent.app'))
    parser.add_argument('--signer', required=True)
    args = parser.parse_args()
    source = args.bundle.resolve()
    with tempfile.TemporaryDirectory(prefix='rv-recovery-', dir='/tmp') as temporary:
        root = Path(temporary).resolve()
        bundle = root / 'Removent.app'
        run('/usr/bin/ditto', str(source), str(bundle))
        version = plistlib.loads((bundle / 'Contents/Info.plist').read_bytes())['RemoventReleaseVersion']
        data = root / 'data'
        data.mkdir()
        (data / 'settings.toml').write_text('host_enabled = false\nupdate_check_enabled = false\nupdate_auto_install = false\n')
        env = dict(os.environ, REMOVENT_DATA_DIR=str(data))
        updater = bundle / 'Contents/MacOS/removent-update'
        cli = bundle / 'Contents/MacOS/removent-cli'
        digest = hashlib.sha256(os.fsencode(data)).hexdigest()[:12]
        agent_label = f'com.alkinum.removent.update.{digest}'
        host_label = f'com.alkinum.removent.daemon.{digest}'
        agent_target = f'gui/{os.getuid()}/{agent_label}'
        host_target = f'gui/{os.getuid()}/{host_label}'
        directory = data / 'update-agent'
        runtime = directory / 'runtime.json'
        heartbeat = directory / 'worker-heartbeat.json'
        bundle_key = hashlib.sha256(os.fsencode(bundle)).hexdigest()
        global_marker = Path.home() / 'Library/Application Support/removent/update-locks' / f'{bundle_key}.maintenance'
        local_marker = data / 'run/update-maintenance'
        observed_workers = set()

        def worker():
            value = read(heartbeat, {})
            pid = value.get('pid')
            if pid and alive(pid):
                observed_workers.add(pid)
                return pid
            return None

        def sign(path, entitlements=None):
            command = ['/usr/bin/codesign', '--force', '--options', 'runtime', '--timestamp=none',
                       '--sign', args.signer, '--identifier', 'com.alkinum.removent.update']
            if entitlements:
                command += ['--entitlements', str(entitlements)]
            run(*command, str(path))

        def queue_worker(path):
            run('/usr/bin/codesign', '--verify', '--strict', str(path))
            run(str(path), '--self-test')
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            shutil.copy2(path, directory / 'workers' / digest)
            write(directory / 'pending-worker.json', digest)
            return digest

        try:
            run(str(updater), '--install', str(bundle), env=env)
            initial = hashlib.sha256(updater.read_bytes()).hexdigest()
            pinned = directory / 'removent-update'
            wait_for(lambda: read(runtime, {}).get('current') == initial,
                     'The initial signed worker did not pass its startup observation', 100)
            supervisor = job_pid(agent_target)
            assert supervisor and worker() != supervisor
            assert hashlib.sha256(pinned.read_bytes()).hexdigest() == initial
            print('PASS: independent supervisor and signed worker remain available with desktop and host closed', flush=True)

            run(str(cli), 'daemon', 'start', env=env)
            old_health = wait_for(lambda: health(data), 'Fixture host did not start')
            old_worker = worker()
            # Stop both update processes at a known idle boundary, then emulate
            # a power/process failure after the old bundle's durable rename.
            os.kill(supervisor, signal.SIGSTOP)
            os.kill(old_worker, signal.SIGSTOP)
            run('/bin/launchctl', 'bootout', host_target)
            wait_for(lambda: not alive(old_health['pid']), 'Old fixture host did not stop')
            backup = bundle.with_suffix('.app.update-backup')
            incoming = bundle.with_suffix('.app.update-incoming')
            write(directory / 'journal.json', dict(phase='Installing', version='9.9.1', previous=version,
                  bundle=str(bundle), incoming=str(incoming), backup=str(backup), resume_host=True,
                  resume_tray=False, reopen=False, baseline=old_health))
            write(global_marker, str(data))
            local_marker.write_text('recovery required\n')
            os.rename(bundle, backup)
            os.kill(old_worker, signal.SIGKILL)
            os.kill(supervisor, signal.SIGCONT)
            restored = wait_for(lambda: health(data) if bundle.exists() and not local_marker.exists() else None,
                                'Independent agent did not restore the missing app and restart host', 70)
            assert restored['version'] == version and restored['pid'] != old_health['pid']
            assert restored['status']['fp_short'] == old_health['status']['fp_short']
            assert not global_marker.exists()
            assert '9.9.1' in read(directory / 'rejected.json', [])
            print('PASS: killed installer with app path missing restores signed backup, identity and running host', flush=True)

            run(str(cli), 'daemon', 'stop', env=env)
            assert (data / 'run/service-stopped').exists()
            code = root / 'bad-worker.c'
            code.write_text('#include <string.h>\nint main(int argc,char**argv){return argc==2&&!strcmp(argv[1],"--self-test")?0:42;}\n')
            bad = root / 'bad-worker'
            run('/usr/bin/xcrun', 'clang', '-Os', str(code), '-o', str(bad))
            sign(bad)
            bad_digest = queue_worker(bad)
            wait_for(lambda: bad_digest in read(runtime, {}).get('rejected', []), 'Bad update worker was not rejected', 55)
            wait_for(worker, 'Known-good recovery worker did not return')
            assert hashlib.sha256(pinned.read_bytes()).hexdigest() == initial
            assert not job_pid(host_target)
            print('PASS: signed updater that crashes after self-test falls back without clearing manual host stop', flush=True)

            # Same real executable, separately signed immutable worker identity.
            # This exercises replacing a failed update engine without replacing
            # the pinned supervisor or installing a fake public release.
            good = root / 'good-worker'
            shutil.copy2(updater, good)
            entitlements = root / 'worker-entitlements.plist'
            entitlements.write_bytes(plistlib.dumps({'com.apple.security.get-task-allow': False}))
            sign(good, entitlements)
            good_digest = queue_worker(good)
            assert good_digest != initial
            wait_for(lambda: read(runtime, {}).get('current') == good_digest,
                     'Healthy repair worker was not promoted', 140)
            assert hashlib.sha256(pinned.read_bytes()).hexdigest() == initial
            old_worker = worker()
            supervisor = job_pid(agent_target)
            os.kill(supervisor, signal.SIGKILL)
            wait_for(lambda: worker() and worker() != old_worker and job_pid(agent_target) != supervisor,
                     'Supervisor restart left an orphan or lost the recovery channel', 45)
            assert not job_pid(host_target)
            print('PASS: a healthy repair worker is promoted; supervisor death recovers without reviving a stopped host', flush=True)
        finally:
            for target in (agent_target, host_target):
                subprocess.run(['/bin/launchctl', 'bootout', target], capture_output=True)
            time.sleep(2)
            for pid in observed_workers:
                process = subprocess.run(['/bin/ps', '-ww', '-p', str(pid), '-o', 'comm='], capture_output=True, text=True)
                if process.stdout.strip().startswith(str(directory) + '/'):
                    os.kill(pid, signal.SIGKILL)
            for label in (agent_label, host_label):
                (Path.home() / f'Library/LaunchAgents/{label}.plist').unlink(missing_ok=True)
            global_marker.unlink(missing_ok=True)
            global_marker.with_suffix('').unlink(missing_ok=True)


if __name__ == '__main__':
    main()
