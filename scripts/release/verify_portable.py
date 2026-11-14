#!/usr/bin/env python3
"""Check portable archives against the release version and exact source commit."""
import hashlib
import json
from pathlib import Path, PurePosixPath
import subprocess
import tarfile
import zipfile

from release_meta import VERSION

ROOT = Path(__file__).resolve().parents[2]
ASSETS = {
    'windows-x86_64': 'Removent-windows-x86_64.zip',
    'linux-x86_64': 'Removent-linux-x86_64.tar.gz',
}


def verify(directory, version, revision):
    for platform, name in ASSETS.items():
        path = directory / name
        with path.open('rb') as source:
            digest = hashlib.file_digest(source, 'sha256').hexdigest()
        checksum = path.with_name(name + '.sha256').read_text().split()
        if checksum != [digest, name]:
            raise ValueError(f'Checksum mismatch: {name}')
        prefix = 'Removent-' + platform
        files = {}
        if name.endswith('.zip'):
            with zipfile.ZipFile(path) as archive:
                for item in archive.infolist():
                    if item.is_dir():
                        continue
                    linked = ((item.external_attr >> 16) & 0o170000) == 0o120000
                    if item.filename in files or linked:
                        raise ValueError(f'Duplicate or linked archive entry: {item.filename}')
                    files[item.filename] = (archive.read(item), 0o755)
        else:
            with tarfile.open(path, 'r:gz') as archive:
                for item in archive:
                    if item.isdir():
                        continue
                    if not item.isfile() or item.name in files:
                        raise ValueError(f'Duplicate or linked archive entry: {item.name}')
                    files[item.name] = (archive.extractfile(item).read(), item.mode)
        for entry in files:
            parts = PurePosixPath(entry).parts
            if len(parts) != 2 or parts[0] != prefix or '\\' in entry or '..' in parts:
                raise ValueError(f'Unexpected archive entry: {entry}')

        def content(file):
            return files[f'{prefix}/{file}'][0]

        metadata = json.loads(content('BUILD.json'))
        if metadata != {'version': version, 'platform': platform, 'source_commit': revision}:
            raise ValueError(f'Wrong release provenance: {name}: {metadata}')
        for document in ('README.md', 'VALIDATION.md', 'LICENSE'):
            if not content(document):
                raise ValueError(f'Empty document: {document}')
        windows = platform.startswith('windows')
        for binary in ('removent', 'removentd', 'removent-tray'):
            binary += '.exe' if windows else ''
            data, mode = files[f'{prefix}/{binary}']
            if windows:
                offset = int.from_bytes(data[60:64], 'little')
                valid = (data[:2] == b'MZ' and offset >= 64
                         and data[offset:offset + 6] == b'PE\x00\x00\x64\x86')
            else:
                valid = (data[:6] == b'\x7fELF\x02\x01' and data[18:20] == b'\x3e\x00'
                         and mode & 0o111 == 0o111)
            if not valid:
                raise ValueError(f'Wrong executable format or permissions: {binary}')
        if windows:
            for runtime in ('vcruntime140.dll', 'vcruntime140_1.dll', 'msvcp140.dll', 'WINDOWS-RUNTIME.txt'):
                if not content(runtime):
                    raise ValueError(f'Empty Windows runtime: {runtime}')
        print(f'Portable release verified: {version} {platform} {revision}')


if __name__ == '__main__':
    revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    verify(ROOT / 'dist/portable', VERSION, revision)
