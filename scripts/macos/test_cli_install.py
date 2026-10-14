"""Run the bundled installer and real shells in disposable user directories."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
INSTALLER = ROOT / 'apps/installer/install-cli.sh'


class CliInstallTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='removent cli ')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.home = self.root / "user's home $() `x` \\"
        self.home.mkdir()
        self.app = self.root / "User's Applications" / 'Removent.app'
        self.cli = self.app / 'Contents/MacOS/removent-cli'
        self.cli.parent.mkdir(parents=True)
        self.cli.write_text('#!/bin/sh\nprintf "cli fixture %s\\n" "$*"\n')
        self.cli.chmod(0o755)
        self.bin = self.home / '.local/bin'
        self.env = {**os.environ, 'HOME': str(self.home), 'PATH': '/usr/bin:/bin',
                    'SHELL': '/bin/zsh'}
        for key in ('ZDOTDIR', 'XDG_CONFIG_HOME', 'BASH_ENV', 'ENV'):
            self.env.pop(key, None)

    def install(self, success=True, app=None, directory=None):
        result = subprocess.run(['/bin/bash', str(INSTALLER), str(app or self.app),
                                 str(directory or self.bin)], env=self.env,
                                capture_output=True, text=True)
        if success:
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stderr, '')
        else:
            self.assertNotEqual(result.returncode, 0, result.stdout)
        return result

    def shell(self, executable, flags, command='removent --version; removent-cli daemon status'):
        result = subprocess.run([executable, *flags, command], env=self.env,
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, 'cli fixture --version\ncli fixture daemon status\n')

    def test_both_commands_in_new_login_and_interactive_shells(self):
        self.install()
        self.shell('/bin/bash', ['-lc'])
        self.shell('/bin/bash', ['-ic'])
        if Path('/bin/zsh').exists():
            self.shell('/bin/zsh', ['-c'])
            self.shell('/bin/zsh', ['-lc'])
            self.shell('/bin/zsh', ['-ic'])
        for name in ('removent', 'removent-cli'):
            self.assertEqual((self.bin / name).resolve(), self.cli.resolve())

    @unittest.skipUnless(shutil.which('fish'), 'fish not installed')
    def test_fish_starts_with_both_commands(self):
        self.install()
        self.shell(shutil.which('fish'), ['-c'])

    def test_repeat_install_preserves_profiles_and_one_path_entry(self):
        profile = self.home / '.zshenv'
        profile.write_text('# My existing configuration\nexport MY_SETTING=kept\n')
        self.install()
        first = {p: p.read_bytes() for p in self.home.rglob('*') if p.is_file() and not p.is_symlink()}
        self.install()
        self.assertEqual({p: p.read_bytes() for p in first}, first)
        self.assertEqual(profile.read_text().count('# Removent command line'), 1)
        command = '. "$HOME/.config/removent/path.sh"; . "$HOME/.config/removent/path.sh"; printf "%s" "$PATH"'
        result = subprocess.check_output(['/bin/bash', '-c', command], env=self.env, text=True)
        self.assertEqual(result.split(':').count(str(self.bin.resolve())), 1)

    def test_existing_files_directories_and_foreign_symlinks_are_preserved(self):
        self.bin.mkdir(parents=True)
        conflict = self.bin / 'removent-cli'
        for kind in ('file', 'directory', 'symlink', 'broken-symlink'):
            with self.subTest(kind=kind):
                if kind == 'file':
                    conflict.write_text('my command')
                elif kind == 'directory':
                    conflict.mkdir()
                else:
                    conflict.symlink_to('/bin/sh' if kind == 'symlink' else '/missing/tool')
                result = self.install(success=False)
                self.assertIn('Existing command preserved', result.stderr)
                self.assertFalse((self.bin / 'removent').exists())
                self.assertFalse((self.home / '.zprofile').exists())
                if kind == 'file':
                    self.assertEqual(conflict.read_text(), 'my command')
                if kind == 'directory':
                    conflict.rmdir()
                else:
                    conflict.unlink()

    def test_existing_command_elsewhere_in_path_is_not_shadowed(self):
        other = self.root / 'other-bin'
        other.mkdir()
        command = other / 'removent'
        command.write_text('#!/bin/sh\nexit 0\n')
        command.chmod(0o755)
        self.env['PATH'] = str(other) + ':' + self.env['PATH']
        self.assertIn(str(command), self.install(success=False).stderr)
        self.assertFalse((self.bin / 'removent-cli').exists())

    def test_upgrade_and_rollback_keep_commands_pointing_at_stable_bundle(self):
        self.install()
        backup = self.app.with_suffix('.app.old')
        self.app.rename(backup)
        self.cli.parent.mkdir(parents=True)
        self.cli.write_text('#!/bin/sh\nprintf "updated\\n"\n')
        self.cli.chmod(0o755)
        command = self.bin / 'removent'
        self.assertEqual(subprocess.check_output([command], text=True), 'updated\n')
        shutil.rmtree(self.app)
        backup.rename(self.app)
        self.assertEqual(subprocess.check_output([command], text=True), 'cli fixture \n')

    def test_moving_app_repairs_both_owned_links(self):
        self.install()
        moved = self.root / 'Other Applications' / 'Removent.app'
        moved.parent.mkdir()
        self.app.rename(moved)
        self.install(app=moved)
        for name in ('removent', 'removent-cli'):
            self.assertEqual((self.bin / name).resolve(), (moved / 'Contents/MacOS/removent-cli').resolve())

    def test_zdotdir_xdg_and_existing_bash_login_are_respected(self):
        zsh = self.home / 'my zsh'
        xdg = self.home / 'my config'
        self.env.update(ZDOTDIR=str(zsh), XDG_CONFIG_HOME=str(xdg))
        (self.home / '.bash_profile').write_text('export BASH_SETTING=kept\n')
        self.install()
        self.assertFalse((self.home / '.zshenv').exists())
        self.assertTrue((zsh / '.zshenv').is_file())
        self.assertTrue((xdg / 'fish/conf.d/removent.fish').is_file())
        self.assertFalse((self.home / '.profile').exists())
        self.shell('/bin/bash', ['-lc'])
        if Path('/bin/zsh').exists():
            self.shell('/bin/zsh', ['-lc'])

    def test_symlinked_dotfiles_are_preserved(self):
        dotfile = self.home / 'managed-zshenv'
        dotfile.write_text('# managed in dotfiles repository\n')
        profile = self.home / '.zshenv'
        profile.symlink_to(dotfile)
        self.install()
        self.assertTrue(profile.is_symlink())
        self.assertIn('# managed in dotfiles repository', dotfile.read_text())
        self.assertIn('# Removent command line', dotfile.read_text())

    def test_invalid_profile_does_not_install_commands(self):
        (self.home / '.bashrc').mkdir()
        self.install(success=False)
        self.assertFalse((self.bin / 'removent').exists())
        self.assertFalse((self.home / '.zprofile').exists())
        self.assertFalse((self.bin / '.removent-cli-install.lock').exists())

    def test_custom_directory_changes_update_managed_path(self):
        self.install()
        custom = self.home / 'my commands'
        self.install(directory=custom)
        self.shell('/bin/bash', ['-lc'])
        result = subprocess.check_output(['/bin/bash', '-lc', 'command -v removent'], env=self.env, text=True)
        self.assertEqual(result.strip(), str(custom.resolve() / 'removent'))


if __name__ == '__main__':
    unittest.main()
