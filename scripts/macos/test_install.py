#!/usr/bin/env python3
"""Installer transaction regressions with isolated macOS and release fixtures."""
import hashlib
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "install.sh"
ASSET = "Removent-0.1.4-macos-arm64.dmg"

MOCK = r'''#!/usr/bin/env python3
import json, os, pathlib, plistlib, shutil, signal, sys
root = pathlib.Path(os.environ['INSTALL_TEST_ROOT'])
name = pathlib.Path(sys.argv[0]).name
args = sys.argv[1:]
def record(value):
    with (root / 'calls').open('a') as stream: stream.write(value + '\n')
def flag(value): return os.environ.get('INSTALL_TEST_' + value) == '1'
if name == 'uname': print('Darwin' if args == ['-s'] else os.environ.get('INSTALL_TEST_ARCH', 'arm64'))
elif name == 'sw_vers': print(os.environ.get('INSTALL_TEST_OS', '26.0'))
elif name == 'id': print('501')
elif name == 'curl':
    assert args[args.index('--proto')+1] == '=https'
    assert args[args.index('--proto-redir')+1] == '=https'
    url = args[-1]
    if url.endswith('/latest'): print('https://github.com/backrunner/removent/releases/tag/v0.1.4', end='')
    else:
        record('download ' + url)
        src = root / url.rsplit('/', 1)[1]
        if not src.exists(): sys.exit(22)
        shutil.copyfile(src, args[args.index('--output')+1])
elif name in ('diskutil', 'hdiutil'):
    if name == 'diskutil' and args == ['image', 'attach', '--help']:
        sys.exit(1 if flag('LEGACY_DISKUTIL') else 0)
    record(name + ' ' + ' '.join(args))
    attaching = args[0] == 'attach' or args[:2] == ['image', 'attach']
    record('mount ' + ('attach' if attaching else 'detach'))
    if attaching:
        if flag('BAD_ATTACH'): sys.exit(1)
        if name == 'diskutil':
            assert args[:6] == ['image', 'attach', '--mountOptions', 'nobrowse', '--readOnly', '--mountPoint']
            option = '--mountPoint'
        else:
            assert args[:4] == ['attach', '-nobrowse', '-readonly', '-mountpoint']
            option = '-mountpoint'
        dst = pathlib.Path(args[args.index(option)+1]); dst.mkdir()
        shutil.copytree(root / 'source.app', dst / 'Removent.app', symlinks=True)
    else:
        assert args[0] == ('eject' if name == 'diskutil' else 'detach')
        if flag('BAD_EJECT'): sys.exit(1)
elif name == 'plutil':
    try:
        raw = pathlib.Path(args[-1]).read_bytes()
        value = json.loads(raw) if raw.startswith(b'{') else plistlib.loads(raw)
        for key in args[args.index('-extract')+1].split('.'): value = value[key]
        fmt = args[args.index('-extract')+2]
        if fmt == 'json': print(json.dumps(value, separators=(',', ':')), end='')
        elif isinstance(value, bool): print(str(value).lower())
        else: print(value)
    except Exception: sys.exit(1)
elif name == 'codesign':
    assert '--verify' in args
    if '-R' in args:
        requirement = args[args.index('-R')+1]
        assert 'PB8H83VL3Z' in requirement and '1.2.840.113635.100.6.1.13' in requirement
        record('signature ' + args[-1])
    if flag('BAD_SIGNATURE'): sys.exit(1)
    if flag('BAD_INSTALLED') and args[-1].startswith(str(root / 'Applications' / 'Removent.app')): sys.exit(1)
elif name == 'spctl':
    record('gatekeeper ' + args[-1])
    if flag('BAD_GATEKEEPER'): sys.exit(1)
elif name == 'ditto':
    record('copy')
    if flag('BAD_COPY'): sys.exit(1)
    shutil.copytree(args[0], args[1], symlinks=True)
elif name == 'ps':
    if flag('UI_RUNNING'): print(str(root / 'Applications/Removent.app/Contents/MacOS/removent'))
    if flag('OTHER_DAEMON'): print(str(root / 'Applications/Removent.app/Contents/MacOS/removentd') + ' --other-data')
elif name == 'launchctl':
    if flag('NO_GUI'): sys.exit(1)
elif name == 'mv':
    if flag('BAD_SWAP') and args[-2].endswith('/new.app'): sys.exit(1)
    if flag('BAD_RESTORE') and args[-2].endswith('/previous.app'): sys.exit(1)
    shutil.move(args[-2], args[-1])
    if (flag('INTERRUPT_BACKUP') and args[-1].endswith('/previous.app')) or (flag('INTERRUPT_SWAP') and args[-2].endswith('/new.app')):
        os.kill(os.getppid(), signal.SIGTERM)
else: raise AssertionError(name)
'''

CLI = r'''#!/usr/bin/env python3
import json, os, pathlib, sys
root = pathlib.Path(os.environ['INSTALL_TEST_ROOT'])
state_file = root / 'state.json'
state = json.loads(state_file.read_text())
action = sys.argv[-1]
with (root / 'calls').open('a') as stream: stream.write('service ' + action + '\n')
if action == 'service-status': print(json.dumps(state))
elif action == 'status': print(json.dumps({'sessions': [1] if os.environ.get('INSTALL_TEST_SESSIONS') == '1' else [], 'host_ready': False}))
elif action == 'stop': state.update(managed=False, reachable=False, stopped_by_user=True)
elif action in ('start', 'login-on'):
    if os.environ.get('INSTALL_TEST_BAD_START') == '1' and (pathlib.Path(__file__).parent / 'new').exists(): sys.exit(1)
    state.update(managed=True, reachable=True, stopped_by_user=False)
    if action == 'login-on': state['launch_at_login'] = True
else: raise AssertionError(action)
state_file.write_text(json.dumps(state))
'''


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='removent installer ')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.mock = self.root / 'mock'; self.mock.mkdir()
        self.app_dir = self.root / 'Applications'; self.app_dir.mkdir()
        self.destination = self.app_dir / 'Removent.app'
        self.env = dict(os.environ, PATH=f"{self.mock}:{os.environ['PATH']}",
                        INSTALL_TEST_ROOT=str(self.root), TMPDIR=str(self.root))
        for key in ('REMOVENT_VERSION', 'REMOVENT_REPO', 'REMOVENT_DATA_DIR'): self.env.pop(key, None)
        for name in ('uname', 'sw_vers', 'id', 'curl', 'diskutil', 'hdiutil', 'plutil', 'codesign', 'spctl', 'ditto', 'ps', 'launchctl', 'mv'):
            file = self.mock / name; file.write_text(MOCK); file.chmod(0o755)
        self.make_app(self.root / 'source.app', new=True)
        (self.root / ASSET).write_bytes(b'release disk image')
        digest = hashlib.sha256((self.root / ASSET).read_bytes()).hexdigest()
        self.checksum = f'{digest}  {ASSET}\n'
        (self.root / 'SHA256SUMS').write_text(self.checksum)
        self.set_state()

    def set_state(self, running=False, stopped=False, login=False):
        state = dict(managed=running, reachable=running, stopped_by_user=stopped, launch_at_login=login)
        (self.root / 'state.json').write_text(json.dumps(state))

    def make_app(self, app, new=False):
        binary = app / 'Contents/MacOS'; binary.mkdir(parents=True)
        cli = binary / 'removent-cli'; cli.write_text(CLI); cli.chmod(0o755)
        daemon = binary / 'removentd'; daemon.write_text('#!/bin/sh\nexit 0\n'); daemon.chmod(0o755)
        (binary / ('new' if new else 'old')).touch()
        (app / 'Contents/Info.plist').write_bytes(plistlib.dumps(dict(CFBundleIdentifier='com.alkinum.removent', RemoventReleaseVersion='0.1.4')))

    def run_install(self, *args):
        return subprocess.run(['bash', str(SCRIPT), '--app-dir', str(self.app_dir), *args],
                              env=self.env, text=True, capture_output=True, timeout=20)

    def calls(self):
        file = self.root / 'calls'
        return file.read_text() if file.exists() else ''

    def assert_clean(self):
        self.assertFalse(any(self.root.glob('removent-install.*')))
        self.assertFalse(any(self.app_dir.glob('.removent-install*')))
        if 'mount attach' in self.calls(): self.assertIn('mount detach', self.calls())

    def assert_old(self, result):
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertTrue((self.destination / 'Contents/MacOS/old').exists(), result.stderr)
        self.assert_clean()

    def test_fresh_install_via_stdin_with_spaces_and_no_service_start(self):
        result = subprocess.run(['bash', '-s', '--', '--app-dir', str(self.app_dir)], input=SCRIPT.read_text(),
                                env=self.env, text=True, capture_output=True, timeout=20)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.destination / 'Contents/MacOS/new').exists())
        self.assertNotIn('service start', self.calls())
        self.assert_clean()

    def test_verify_only_checks_every_component_without_installing(self):
        result = self.run_install('--verify-only')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.destination.exists())
        self.assertEqual(self.calls().count('signature '), 6)
        self.assert_clean()

    def test_diskutil_mount_and_eject_avoid_deprecated_commands(self):
        result = self.run_install('--verify-only')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('diskutil image attach --mountOptions nobrowse --readOnly --mountPoint ', self.calls())
        self.assertIn('diskutil eject ', self.calls())
        self.assertNotIn('hdiutil ', self.calls())
        self.assert_clean()

    def test_legacy_diskutil_uses_hdiutil_for_mount_and_cleanup(self):
        self.env['INSTALL_TEST_LEGACY_DISKUTIL'] = '1'
        result = self.run_install('--verify-only')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('hdiutil attach -nobrowse -readonly -mountpoint ', self.calls())
        self.assertIn('hdiutil detach ', self.calls())
        self.assertNotIn('diskutil eject ', self.calls())
        self.assert_clean()

    def test_failed_attach_does_not_retry_or_replace_existing_app(self):
        self.make_app(self.destination)
        self.env['INSTALL_TEST_BAD_ATTACH'] = '1'
        result = self.run_install()
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue((self.destination / 'Contents/MacOS/old').exists())
        self.assertNotIn('hdiutil ', self.calls())
        self.assertNotIn('mount detach', self.calls())
        self.assertFalse(any(self.root.glob('removent-install.*')))

    def test_failed_eject_retains_mount_directory_and_reports_failure(self):
        self.env['INSTALL_TEST_BAD_EJECT'] = '1'
        result = self.run_install('--verify-only')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Disk image remains mounted', result.stderr)
        retained = list(self.root.glob('removent-install.*/mnt/Removent.app'))
        self.assertEqual(len(retained), 1)
        self.assertFalse(self.destination.exists())

    def test_upgrade_refreshes_running_service_and_preserves_login(self):
        self.make_app(self.destination)
        self.set_state(running=True, login=True)
        result = self.run_install('--version', 'v0.1.4')
        self.assertEqual(result.returncode, 0, result.stderr)
        state = json.loads((self.root / 'state.json').read_text())
        self.assertTrue(state['reachable']); self.assertTrue(state['launch_at_login'])
        self.assertLess(self.calls().index('service stop'), self.calls().index('service start'))
        self.assertFalse((self.destination / 'Contents/MacOS/old').exists())
        self.assert_clean()

    def test_stopped_service_stays_stopped_and_login_is_unchanged(self):
        self.make_app(self.destination)
        self.set_state(stopped=True, login=True)
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn('service stop', self.calls()); self.assertNotIn('service start', self.calls())
        self.assertTrue(json.loads((self.root / 'state.json').read_text())['stopped_by_user'])
        self.assert_clean()

    def test_explicit_start_and_login(self):
        for args, login in [(('--start',), False), (('--login',), True)]:
            self.set_state(stopped=True)
            result = self.run_install(*args)
            self.assertEqual(result.returncode, 0, result.stderr)
            state = json.loads((self.root / 'state.json').read_text())
            self.assertTrue(state['reachable']); self.assertEqual(state['launch_at_login'], login)
            self.assert_clean()

    def test_checksums_missing_duplicate_corrupt_and_missing_download(self):
        self.make_app(self.destination)
        for payload in ['', self.checksum * 2, f"{'0'*64}  {ASSET}\n", f"invalid  {ASSET}\n"]:
            (self.root / 'SHA256SUMS').write_text(payload)
            self.assert_old(self.run_install())
        (self.root / ASSET).unlink()
        self.assert_old(self.run_install())

    def test_signature_gatekeeper_and_copy_failures_keep_old_app(self):
        self.make_app(self.destination)
        for flag in ('BAD_SIGNATURE', 'BAD_GATEKEEPER', 'BAD_COPY'):
            self.env['INSTALL_TEST_' + flag] = '1'
            self.assert_old(self.run_install())
            self.env.pop('INSTALL_TEST_' + flag)

    def test_swap_installed_validation_and_resume_failures_roll_back(self):
        self.make_app(self.destination)
        for flag in ('BAD_SWAP', 'BAD_INSTALLED', 'BAD_START'):
            self.set_state(running=True, login=True)
            self.env['INSTALL_TEST_' + flag] = '1'
            self.assert_old(self.run_install())
            state = json.loads((self.root / 'state.json').read_text())
            self.assertTrue(state['reachable']); self.assertTrue(state['launch_at_login'])
            self.env.pop('INSTALL_TEST_' + flag)

    def test_interrupt_between_move_and_flag_restores_app_and_service(self):
        self.make_app(self.destination)
        for flag in ('INTERRUPT_BACKUP', 'INTERRUPT_SWAP'):
            self.set_state(running=True)
            self.env['INSTALL_TEST_' + flag] = '1'
            self.assert_old(self.run_install())
            self.assertTrue(json.loads((self.root / 'state.json').read_text())['reachable'])
            self.env.pop('INSTALL_TEST_' + flag)

    def test_failed_rollback_retains_previous_bundle_for_manual_recovery(self):
        self.make_app(self.destination)
        self.env['INSTALL_TEST_BAD_INSTALLED'] = '1'
        self.env['INSTALL_TEST_BAD_RESTORE'] = '1'
        result = self.run_install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Restore the previous app', result.stderr)
        backups = list(self.app_dir.glob('.removent-install.*/previous.app'))
        self.assertEqual(len(backups), 1)
        self.assertTrue((backups[0] / 'Contents/MacOS/old').exists())
        self.assertFalse((self.app_dir / '.removent-install.lock').exists())
        self.assertIn('mount detach', self.calls())

    def test_running_ui_or_active_sessions_refuse_without_stopping(self):
        self.make_app(self.destination)
        self.set_state(running=True)
        for flag in ('UI_RUNNING', 'SESSIONS'):
            self.env['INSTALL_TEST_' + flag] = '1'
            self.assert_old(self.run_install())
            self.assertNotIn('service stop', self.calls())
            self.env.pop('INSTALL_TEST_' + flag)

    def test_other_daemon_blocks_swap_and_resumes_own_service(self):
        self.make_app(self.destination)
        self.set_state(running=True)
        self.env['INSTALL_TEST_OTHER_DAEMON'] = '1'
        self.assert_old(self.run_install())
        self.assertTrue(json.loads((self.root / 'state.json').read_text())['reachable'])

    def test_no_gui_allows_install_only_and_blocks_start_before_replacement(self):
        self.env['INSTALL_TEST_NO_GUI'] = '1'
        self.make_app(self.destination)
        self.assert_old(self.run_install('--start'))
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assert_clean()

    def test_symlink_existing_lock_invalid_tag_and_unsupported_system(self):
        self.make_app(self.destination)
        for args in [('--version', 'v0.1.4;false'), ('--verify-only', '--login'), ('--app-dir', 'relative')]:
            self.assert_old(self.run_install(*args))
        for flag, value in [('ARCH', 'x86_64'), ('OS', '25.0')]:
            self.env['INSTALL_TEST_' + flag] = value
            self.assert_old(self.run_install())
            self.env.pop('INSTALL_TEST_' + flag)
        lock = self.app_dir / '.removent-install.lock'; lock.mkdir()
        result = self.run_install(); self.assertNotEqual(result.returncode, 0); self.assertTrue(lock.exists())
        lock.rmdir()
        real = self.root / 'real.app'; self.destination.rename(real); self.destination.symlink_to(real)
        result = self.run_install(); self.assertNotEqual(result.returncode, 0)
        self.assertTrue(self.destination.is_symlink()); self.assertTrue((real / 'Contents/MacOS/old').exists())
        self.assert_clean()

    def test_elevation_is_limited_to_files_and_staging_remains_readable(self):
        sudo = self.mock / 'sudo'
        sudo.write_text(r'''#!/usr/bin/env python3
import os, pathlib, subprocess, sys
root = pathlib.Path(os.environ['INSTALL_TEST_ROOT'])
args = sys.argv[1:]
with (root / 'calls').open('a') as stream: stream.write('sudo ' + ' '.join(args) + '\n')
if args == ['-n', 'true']: sys.exit(0)
assert args[0] == '--'
args = args[1:]
assert args[0] in ('mkdir', 'mktemp', 'chown', 'ditto', 'mv', 'rm', 'rmdir')
if args[0] == 'chown': sys.exit(0)  # Fixture runs as its actual owner already.
os.chmod(root / 'Applications', 0o755)
sys.exit(subprocess.call(args))
''')
        sudo.chmod(0o755)
        self.app_dir.chmod(0o555)
        self.addCleanup(self.app_dir.chmod, 0o755)
        result = self.run_install('--start')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('sudo -- chown 501', self.calls())
        self.assertIn('service start', self.calls())
        self.assert_clean()

    def test_release_version_mismatch_is_rejected(self):
        self.make_app(self.destination)
        file = self.root / 'source.app/Contents/Info.plist'
        value = plistlib.loads(file.read_bytes()); value['RemoventReleaseVersion'] = '9.9.9'
        file.write_bytes(plistlib.dumps(value))
        self.assert_old(self.run_install())


if __name__ == '__main__': unittest.main()
