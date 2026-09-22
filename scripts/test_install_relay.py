#!/usr/bin/env python3
"""Offline installer regressions; all network and installation paths are isolated."""
import hashlib
import io
import os
import pathlib
import subprocess
import tarfile
import tempfile
import unittest

SCRIPT = pathlib.Path(__file__).with_name("install_relay.sh").resolve()
NAME = "removent-relay-v0.1.0-linux-x86_64.tar.gz"


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="relay installer ")
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.mock = self.root / "mock"
        self.mock.mkdir()
        self.env = dict(os.environ, PATH=f"{self.mock}:{os.environ['PATH']}", RELAY_TEST_ROOT=str(self.root))
        self.env.pop("REMOVENT_RELAY_VERSION", None)
        self.executable("uname", '#!/bin/sh\nif [ "$1" = -s ]; then echo Linux; else echo x86_64; fi\n')
        self.executable("curl", '''#!/usr/bin/env python3
import os, pathlib, shutil, sys
root = pathlib.Path(os.environ['RELAY_TEST_ROOT'])
args = sys.argv[1:]
url = args[-1]
base = 'https://github.com/backrunner/removent/releases'
assert '--proto' in args and args[args.index('--proto') + 1] == '=https'
if url == base + '/latest':
    print(base + '/tag/v0.1.0', end='')
else:
    assert url.startswith(base + '/download/v0.1.0/')
    name = url.rsplit('/', 1)[1]
    source = root / name
    if not source.is_file(): sys.exit(22)
    shutil.copyfile(source, args[args.index('--output') + 1])
''')
        self.asset = NAME
        self.archive()

    def executable(self, name, text):
        file = self.mock / name
        file.write_text(text)
        file.chmod(0o755)

    def archive(self, name="removent-relay", version="0.1.0", symlink=False):
        payload = f'#!/bin/sh\nprintf "removent-relay {version}\\n"\n'.encode()
        with tarfile.open(self.root / self.asset, "w:gz") as archive:
            member = tarfile.TarInfo(name)
            member.mode, member.size = 0o755, len(payload)
            if symlink:
                member.type, member.linkname, member.size = tarfile.SYMTYPE, "/bin/sh", 0
                archive.addfile(member)
            else:
                archive.addfile(member, io.BytesIO(payload))
        self.checksum()

    def checksum(self):
        digest = hashlib.sha256((self.root / self.asset).read_bytes()).hexdigest()
        (self.root / (self.asset + ".sha256")).write_text(f"{digest}  {self.asset}\n")

    def run_installer(self, *args):
        return subprocess.run(["sh", str(SCRIPT), "--no-setup", "--bin-dir", str(self.bin), *args], env=self.env,
                              text=True, capture_output=True, timeout=10)

    def existing(self):
        target = self.bin / "removent-relay"
        target.write_text("old installation")
        return target

    def assert_refused(self, result, target):
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertEqual(target.read_text(), "old installation")
        self.assertEqual(sorted(p.name for p in self.bin.iterdir()), ["removent-relay"])

    def test_latest_and_pinned_install_and_atomic_upgrade_with_spaces(self):
        target = self.existing()
        for args in [(), ("--version", "v0.1.0")]:
            result = self.run_installer(*args)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(target.stat().st_mode & 0o777, 0o755)
            self.assertEqual(subprocess.check_output([str(target), "--version"], text=True), "removent-relay 0.1.0\n")
            self.assertEqual(len(list(self.bin.iterdir())), 1)

    def test_macos_universal_installs_on_both_architectures(self):
        self.executable("sw_vers", '#!/bin/sh\necho 13.0\n')
        for architecture in ["arm64", "x86_64"]:
            self.executable("uname", f'#!/bin/sh\nif [ "$1" = -s ]; then echo Darwin; else echo {architecture}; fi\n')
            self.asset = "removent-relay-v0.1.0-macos-universal.tar.gz"
            self.archive()
            result = self.run_installer()
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertNotIn("sudo removent-relay restart", result.stdout)
        self.executable("sw_vers", '#!/bin/sh\necho 12.7\n')
        self.assertNotEqual(self.run_installer().returncode, 0)

    def test_bad_checksum_preserves_existing_installation(self):
        target = self.existing()
        (self.root / (self.asset + ".sha256")).write_text(f"{'0' * 64}  {self.asset}\n")
        self.assert_refused(self.run_installer(), target)

    def test_missing_release_asset_and_version_mismatch_are_fatal(self):
        target = self.existing()
        (self.root / self.asset).unlink()
        self.assert_refused(self.run_installer(), target)
        self.archive(version="9.9.9")
        self.assert_refused(self.run_installer(), target)

    def test_archive_traversal_extra_members_and_symlinks_are_rejected(self):
        target = self.existing()
        self.archive(name="../removent-relay")
        self.assert_refused(self.run_installer(), target)
        self.archive(symlink=True)
        self.assert_refused(self.run_installer(), target)
        self.archive(name="removent-relay\nextra")
        self.assert_refused(self.run_installer(), target)

    def test_invalid_tag_and_symlink_destination_are_rejected(self):
        target = self.existing()
        self.assert_refused(self.run_installer("--version", "v0.1.0;false"), target)
        real = self.root / "real"
        target.rename(real)
        target.symlink_to(real)
        self.assertNotEqual(self.run_installer().returncode, 0)
        self.assertEqual(real.read_text(), "old installation")
        self.assertTrue(target.is_symlink())


if __name__ == "__main__":
    unittest.main()
