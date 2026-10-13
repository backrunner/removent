#!/usr/bin/env python3
"""Opt-in real SSH/release/launchd acceptance on macOS, using private paths.

Requires an active graphical session. Opens a temporary key-only SSH server on
loopback as the current user; never changes Remote Login or the installed app.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shlex
import socket
import subprocess
import tempfile
import time

SCRIPT = Path(__file__).resolve().parents[1] / 'install.sh'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--live', action='store_true', required=True)
    parser.add_argument('--version', default='v0.1.4')
    args = parser.parse_args()
    subprocess.run(['launchctl', 'print', f'gui/{os.getuid()}'], check=True, stdout=subprocess.DEVNULL)
    with tempfile.TemporaryDirectory(prefix='removent-ssh-') as temp:
        root = Path(temp).resolve()
        applications = root / 'Applications'
        data = root / 'data'; data.mkdir()
        (data / 'settings.toml').write_text('host_enabled = false\n')
        label = 'com.alkinum.removent.daemon.' + hashlib.sha256(os.fsencode(data)).hexdigest()[:12]
        login_file = Path.home() / 'Library/LaunchAgents' / (label + '.plist')
        assert not login_file.exists()
        for name in ('host', 'client'):
            subprocess.run(['ssh-keygen', '-q', '-t', 'ed25519', '-N', '', '-f', str(root / name)], check=True)
        (root / 'authorized_keys').write_text((root / 'client.pub').read_text())
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0)); port = sock.getsockname()[1]
        user = subprocess.check_output(['id', '-un'], text=True).strip()
        config = root / 'sshd_config'
        config.write_text(f'Port {port}\nListenAddress 127.0.0.1\nHostKey {root}/host\n'
                          f'PidFile {root}/pid\nAuthorizedKeysFile {root}/authorized_keys\n'
                          'StrictModes no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\n'
                          f'UsePAM no\nAllowUsers {user}\nLogLevel ERROR\n')
        ssh = ['ssh', '-p', str(port), '-i', str(root / 'client'), '-o', 'IdentitiesOnly=yes',
               '-o', 'BatchMode=yes', '-o', 'StrictHostKeyChecking=accept-new',
               '-o', f'UserKnownHostsFile={root}/known_hosts', '-o', 'ConnectTimeout=5', '127.0.0.1']
        cli = applications / 'Removent.app/Contents/MacOS/removent-cli'
        env = dict(os.environ, REMOVENT_DATA_DIR=str(data))
        env.pop('REMOVENT_DEV_SUPERVISED', None)
        def remote(argv, source=None):
            result = subprocess.run(ssh + [shlex.join(['env', f'REMOVENT_DATA_DIR={data}', *map(str, argv)])],
                                    input=source, text=True, capture_output=True, timeout=180)
            assert result.returncode == 0, result.stdout + result.stderr
            return result.stdout
        def state(action):
            return json.loads(remote([cli, 'daemon', action]))
        def pid():
            output = subprocess.check_output(['launchctl', 'print', f'gui/{os.getuid()}/{label}'], text=True)
            return next(int(line.split('=')[1]) for line in output.splitlines() if line.strip().startswith('pid = '))
        with (root / 'sshd.log').open('w') as log:
            server = subprocess.Popen(['/usr/sbin/sshd', '-D', '-e', '-f', str(config)], stdout=log, stderr=log)
            try:
                time.sleep(0.5)
                remote(['true'])
                command = ['bash', '-s', '--', '--version', args.version, '--app-dir', str(applications)]
                source = SCRIPT.read_text()
                print(remote(command + ['--start'], source), flush=True)
                assert state('service-status')['reachable']
                first_pid = pid()
                state('login-on')
                print('PASS: real release installed over SSH; private launchd daemon reachable', flush=True)
                print(remote(command, source), flush=True)
                current = state('service-status')
                assert current['reachable'] and current['launch_at_login'] and not current['stopped_by_user']
                replacement_pid = pid()
                assert replacement_pid != first_pid
                assert 'host_enabled = false' in (data / 'settings.toml').read_text()
                print(f'PASS: SSH upgrade replaced daemon PID {first_pid} -> {replacement_pid} and preserved login/sharing settings', flush=True)
                state('stop')
                print(remote(command, source), flush=True)
                current = state('service-status')
                assert current['stopped_by_user'] and not current['reachable'] and not current['managed']
                assert current['launch_at_login']
                assert not list(applications.glob('.removent-install*'))
                print('PASS: SSH reinstall preserved explicit stop and login preference; staging cleaned', flush=True)
            finally:
                if cli.exists():
                    for action in ('login-off', 'stop'):
                        subprocess.run([str(cli), 'daemon', action], env=env, capture_output=True, timeout=25)
                server.terminate(); server.wait(timeout=10)
                if login_file.exists():
                    raise RuntimeError(f'Private test login registration needs removal: {login_file}')


if __name__ == '__main__': main()
