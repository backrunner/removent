#!/usr/bin/env python3
"""Release startup gate: real packaged GUI/host, disposable data, no TCC prompts."""
import argparse
import json
import os
from pathlib import Path
import plistlib
import signal
import socket
import subprocess
import tempfile
import time


def health(data):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(2)
        stream.connect(str(data / 'run/removentd.sock'))
        stream.sendall(b'{"type":"update_health"}\n')
        with stream.makefile('rb') as reader:
            for _ in range(64):
                reply = json.loads(reader.readline(1024 * 1024))
                if reply.get('type') == 'update_health':
                    return reply
    raise AssertionError('Host did not return update health')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bundle', type=Path, default=Path('dist/Removent.app'))
    args = parser.parse_args()
    bundle = args.bundle.resolve()
    version = plistlib.loads((bundle / 'Contents/Info.plist').read_bytes())['RemoventReleaseVersion']
    with tempfile.TemporaryDirectory(prefix='rv-start-', dir='/tmp') as temporary:
        data = Path(temporary)
        (data / 'settings.toml').write_text('host_enabled = false\nupdate_check_enabled = false\nupdate_auto_install = false\n')
        env = dict(os.environ, REMOVENT_DATA_DIR=str(data))
        result = subprocess.run([bundle / 'Contents/MacOS/removent', '--update-smoke'],
                                env=env, capture_output=True, text=True, timeout=40)
        assert result.returncode == 0, result.stderr
        assert (data / 'run/update-smoke-ok').read_text().strip() == version
        print(f'PASS: packaged desktop {version} rendered a frame, survived ten seconds, and exited cleanly', flush=True)
        with (data / 'host-output.log').open('w') as log:
            host = subprocess.Popen([bundle / 'Contents/MacOS/removentd'], env=env, stdout=log, stderr=log)
            try:
                deadline = time.monotonic() + 15
                while True:
                    assert host.poll() is None, 'Packaged host crashed on startup'
                    try:
                        reply = health(data)
                        break
                    except (OSError, ValueError):
                        assert time.monotonic() < deadline, 'Packaged host did not become responsive'
                        time.sleep(0.1)
                for _ in range(10):
                    reply = health(data)
                    assert reply['version'] == version and reply['pid'] == host.pid
                    assert reply['status']['running'] is False
                    assert host.poll() is None
                    time.sleep(1)
                host.send_signal(signal.SIGTERM)
                assert host.wait(timeout=10) == 0
                print(f'PASS: packaged host {version} answered real IPC for ten seconds and shut down cleanly', flush=True)
            finally:
                if host.poll() is None:
                    host.kill()
                    host.wait()
        result = subprocess.run([bundle / 'Contents/MacOS/removent-update', '--self-test'],
                                capture_output=True, text=True, timeout=10, check=True)
        assert version in result.stdout
        print('PASS: independent updater starts without the desktop or host', flush=True)


if __name__ == '__main__':
    main()
