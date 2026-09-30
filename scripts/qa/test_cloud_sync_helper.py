#!/usr/bin/env python3
"""Exercise the ad-hoc helper's real lifecycle with isolated local storage.

This deliberately requires an unprovisioned bundle: it never calls CloudKit.
"""
import argparse
import json
import os
import pathlib
import subprocess
import tempfile
import time


def wait_for(predicate, message, timeout=20):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.2)
    raise AssertionError(message)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--bundle', type=pathlib.Path, default=pathlib.Path('dist/Removent.app'))
    args = parser.parse_args()
    bundle = args.bundle.resolve()
    helper = bundle / 'Contents/Helpers/RemoventSync.app'
    assert not (helper / 'Contents/embedded.provisionprofile').exists(), 'Use an ad-hoc development bundle'
    from verify_release import verify_platform
    verify_platform(bundle)
    print('PASS: app, tray, sync helper and bundled binaries require macOS 26', flush=True)
    cli = bundle / 'Contents/MacOS/removent-cli'
    executable = helper / 'Contents/MacOS/RemoventSync'
    children = []
    with tempfile.TemporaryDirectory(prefix='removent-sync-lifecycle-') as directory:
        environment = dict(os.environ, REMOVENT_DATA_DIR=directory)

        def command(op, **fields):
            output = subprocess.check_output([str(cli), 'cloud-sync'],
                input=json.dumps(dict(op=op, **fields)).encode(), env=environment)
            result = json.loads(output)
            assert result['ok'], result
            return result['value']

        def launch(parent):
            process = subprocess.Popen([str(executable), '--data-dir', directory, '--parent-pid', str(parent)],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            children.append(process)
            return process

        try:
            command('enable', enabled=True)
            first = launch(os.getpid())
            wait_for(lambda: command('status')['code'] == 'configuration', 'Missing configuration status')
            assert first.poll() is None
            second = launch(os.getpid())
            assert second.wait(timeout=10) == 0
            assert first.poll() is None
            print('PASS: unprovisioned helper reports configuration; one owner per data directory', flush=True)
            command('enable', enabled=False)
            assert first.wait(timeout=20) == 0
            print('PASS: disabling sync exits the helper and releases its lock', flush=True)
            parent = subprocess.Popen(['/bin/sleep', '60'])
            children.append(parent)
            command('enable', enabled=True)
            restarted = launch(parent.pid)
            wait_for(lambda: command('status')['code'] == 'configuration', 'Helper did not restart')
            parent.terminate(); parent.wait(timeout=5)
            assert restarted.wait(timeout=10) == 0
            print('PASS: restarted helper exits when its parent closes', flush=True)
            assert not (pathlib.Path(directory) / 'identity.json').exists()
        finally:
            for process in children:
                if process.poll() is None:
                    process.terminate()
                process.wait(timeout=10)


if __name__ == '__main__':
    main()
