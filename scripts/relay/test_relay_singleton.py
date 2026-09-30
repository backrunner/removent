#!/usr/bin/env python3
"""macOS relay process lock across QUIC/WebSocket, ports, and config aliases."""
import argparse
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=Path('target/debug/removent-relay'))
    args = parser.parse_args()
    binary = args.binary.resolve()
    with tempfile.TemporaryDirectory(prefix='relay-singleton-') as tmp:
        root = Path(tmp)
        base = f'''identity_dir = "{root / 'identity'}"
max_connections = 8
max_clients_per_room = 2
max_bytes_per_second = 100000
[[rooms]]
name = "test"
host_token_sha256 = "{'11' * 32}"
client_token_sha256 = "{'22' * 32}"
'''
        configs = []
        for name in ('one', 'two'):
            config = root / f'{name}.toml'
            config.write_text('listen = "127.0.0.1:0"\n' + base)
            config.chmod(0o600)
            configs.append(config)
        first = None
        ready_file = root / 'ready'
        env = dict(os.environ, REMOVENT_RELAY_READY_FILE=str(ready_file))

        def wait_ready(process):
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                assert process.poll() is None, process.stderr.read()
                if ready_file.exists() and ready_file.read_text() == str(process.pid):
                    return
                time.sleep(0.05)
            raise AssertionError('Relay did not become ready')

        try:
            # Each config requests an ephemeral port, so bind() alone cannot
            # detect duplicate identities or competing transport types.
            first = subprocess.Popen([binary, 'serve', configs[0]], env=env,
                                     stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
            wait_ready(first)
            for command in ('serve', 'serve-websocket'):
                result = subprocess.run([binary, command, configs[1]], env=env, capture_output=True, text=True, timeout=5)
                assert result.returncode != 0 and 'already running' in result.stderr, result.stderr
                assert first.poll() is None
            os.kill(first.pid, signal.SIGKILL)
            first.wait(timeout=5)
            first = subprocess.Popen([binary, 'serve-websocket', configs[1]], env=env,
                                     stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
            wait_ready(first)
            print('PASS: one relay per identity across configs/transports; crash releases lock')
        finally:
            if first and first.poll() is None:
                first.terminate()
                first.wait(timeout=5)


if __name__ == '__main__':
    main()
