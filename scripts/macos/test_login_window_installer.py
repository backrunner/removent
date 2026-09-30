import importlib.util
import json
import plistlib
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('installer', Path(__file__).with_name('install_login_window_host.py'))
installer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(installer)


class InstallerTests(unittest.TestCase):
    def test_publish_is_complete_and_collision_rolls_back_only_new_bundle(self):
        with tempfile.TemporaryDirectory() as directory:
            parent = Path(directory)
            staged = parent / 'staged'
            staged.mkdir()
            (staged / 'fixture').write_bytes(b'complete')
            root = parent / 'installed'
            plist = parent / 'agent.plist'
            definition = {'Label': 'test.fixture'}
            installer.publish_install(staged, root, plist, definition)
            self.assertEqual(plistlib.loads(plist.read_bytes()), definition)
            self.assertEqual((root / 'fixture').read_bytes(), b'complete')
            self.assertFalse(staged.exists())
            staged.mkdir()
            second_root = parent / 'second'
            with self.assertRaises(FileExistsError):
                installer.publish_install(staged, second_root, plist, {'Label': 'replacement'})
            self.assertFalse(second_root.exists())
            self.assertEqual(plistlib.loads(plist.read_bytes()), definition)
            self.assertEqual((root / 'fixture').read_bytes(), b'complete')
            self.assertEqual(list(parent.glob('.removent-loginwindow-*')), [])

    def test_snapshot_drops_untrusted_and_requires_input_grant(self):
        granted = {'trusted': True, 'granted_caps': {'video': True, 'input': True}, 'fingerprint': 'fixture'}
        rejected = {'trusted': False, 'granted_caps': {'video': True, 'input': True}}
        self.assertEqual(json.loads(installer.trusted_snapshot(json.dumps([granted, rejected]).encode())), [granted])
        for records in [[], [rejected], [{'trusted': True, 'granted_caps': {'video': True}}]]:
            with self.assertRaises(ValueError):
                installer.trusted_snapshot(json.dumps(records).encode())

    def test_source_symlink_and_oversized_file_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'regular'
            path.write_bytes(b'fixture')
            self.assertEqual(installer.read_regular(path), b'fixture')
            link = Path(directory) / 'link'
            link.symlink_to(path)
            with self.assertRaises(OSError):
                installer.read_regular(link)
            path.write_bytes(b'x' * 1_048_577)
            with self.assertRaises(ValueError):
                installer.read_regular(path)


if __name__ == '__main__':
    unittest.main()
