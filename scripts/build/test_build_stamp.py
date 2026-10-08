import contextlib
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import build_stamp


class BuildStampTests(unittest.TestCase):
    def test_source_fingerprint_detects_edits_additions_and_deletions(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'Cargo.toml').write_text('original')
            with patch.object(build_stamp, 'ROOT', root), patch.object(
                    build_stamp.subprocess, 'check_output', return_value=b'Cargo.toml\0'):
                before = build_stamp.source_digest()
                (root / 'Cargo.toml').write_text('updated')
                self.assertNotEqual(before, build_stamp.source_digest())
                (root / 'Cargo.toml').unlink()
                self.assertNotEqual(before, build_stamp.source_digest())

    def test_stale_bundle_is_rejected_and_current_bundle_is_identified(self):
        with tempfile.TemporaryDirectory() as tmp:
            app = Path(tmp) / 'Removent.app'
            resources = app / 'Contents/Resources'
            resources.mkdir(parents=True)
            (resources / 'build-info.json').write_text(json.dumps({'source_sha256': 'old'}))
            with patch('sys.argv', ['build_stamp', 'verify', '--app', str(app)]), patch.object(
                    build_stamp, 'source_digest', return_value='current'):
                with self.assertRaisesRegex(SystemExit, 'older than'):
                    build_stamp.main()
                (resources / 'build-info.json').write_text(json.dumps({'source_sha256': 'current'}))
                with contextlib.redirect_stdout(io.StringIO()) as output:
                    build_stamp.main()
                self.assertEqual(json.loads(output.getvalue())['source_sha256'], 'current')

    def test_changes_during_build_prevent_stamp_creation(self):
        with tempfile.TemporaryDirectory() as tmp:
            with patch('sys.argv', ['build_stamp', 'write', '--app', tmp, '--expected', 'before']), patch.object(
                    build_stamp, 'source_digest', return_value='after'):
                with self.assertRaisesRegex(SystemExit, 'changed during'):
                    build_stamp.main()
            self.assertFalse((Path(tmp) / 'Contents/Resources/build-info.json').exists())


if __name__ == '__main__':
    unittest.main()
