import hashlib
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
import zipfile

from verify_portable import ASSETS, verify


class PortableReleaseTests(unittest.TestCase):
    def packages(self, directory, *, revision='a' * 40, mode=0o755, extra=None):
        for platform, name in ASSETS.items():
            windows = platform.startswith('windows')
            root = 'Removent-' + platform
            binary = bytearray(128)
            if windows:
                binary[:2] = b'MZ'
                binary[60:64] = (64).to_bytes(4, 'little')
                binary[64:70] = b'PE\x00\x00\x64\x86'
            else:
                binary[:6] = b'\x7fELF\x02\x01'
                binary[18:20] = b'\x3e\x00'
            files = {f'{root}/{n}' + ('.exe' if windows else ''): bytes(binary)
                     for n in ('removent', 'removentd', 'removent-tray')}
            files.update({f'{root}/{n}': b'document' for n in ('README.md', 'VALIDATION.md', 'LICENSE')})
            files[f'{root}/BUILD.json'] = json.dumps({
                'version': '0.2.0', 'platform': platform, 'source_commit': revision,
            }).encode()
            if windows:
                files.update({f'{root}/{n}': b'runtime' for n in
                              ('vcruntime140.dll', 'vcruntime140_1.dll', 'msvcp140.dll', 'WINDOWS-RUNTIME.txt')})
            if extra:
                files.update(extra)
            path = directory / name
            if windows:
                with zipfile.ZipFile(path, 'w') as archive:
                    for entry, data in files.items():
                        archive.writestr(entry, data)
            else:
                with tarfile.open(path, 'w:gz') as archive:
                    for entry, data in files.items():
                        item = tarfile.TarInfo(entry)
                        item.size, item.mode = len(data), mode
                        archive.addfile(item, io.BytesIO(data))
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            path.with_name(name + '.sha256').write_text(f'{digest}  {name}\n')

    def test_accepts_matching_release_and_rejects_wrong_version_or_commit(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.packages(directory)
            verify(directory, '0.2.0', 'a' * 40)
            for version, revision in [('0.1.12', 'a' * 40), ('0.2.0', 'b' * 40)]:
                with self.subTest(version=version, revision=revision), self.assertRaisesRegex(ValueError, 'provenance'):
                    verify(directory, version, revision)

    def test_rejects_corrupted_package(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.packages(directory)
            (directory / ASSETS['windows-x86_64']).write_bytes(b'corrupt')
            with self.assertRaisesRegex(ValueError, 'Checksum'):
                verify(directory, '0.2.0', 'a' * 40)

    def test_rejects_unexecutable_linux_or_escaping_entries(self):
        for options, message in [({'mode': 0o644}, 'permissions'),
                                 ({'extra': {'../escape': b'bad'}}, 'archive entry')]:
            with self.subTest(options=options), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                self.packages(directory, **options)
                with self.assertRaisesRegex(ValueError, message):
                    verify(directory, '0.2.0', 'a' * 40)


if __name__ == '__main__':
    unittest.main()
