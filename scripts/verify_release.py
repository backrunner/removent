#!/usr/bin/env python3
"""Fail closed before uploading: metadata, hashes, signatures and notarization."""
import hashlib
import json
import pathlib
import plistlib
import subprocess
import tempfile
import zipfile
from gen_latest import OPENSSL, signing_payload, verify_payload
from release_meta import VERSION, BASE

ROOT = pathlib.Path(__file__).resolve().parent.parent
DIST = ROOT / 'dist'
TEAM = 'PB8H83VL3Z'
REQUIREMENT = f'anchor apple generic and identifier "io.removent.app" and certificate leaf[subject.OU] = "{TEAM}" and certificate leaf[field.1.2.840.113635.100.6.1.13] exists'

def run(*args):
    return subprocess.run(args, check=True, capture_output=True).stdout


def verify_bundle(bundle):
    p = plistlib.loads((bundle / 'Contents/Info.plist').read_bytes())
    assert p['RemoventReleaseVersion'] == VERSION
    assert p['CFBundleShortVersionString'] == BASE
    assert p['CFBundleIdentifier'] == 'io.removent.app'
    assert p['CFBundleIconFile'] == 'AppIcon'
    assert (bundle / 'Contents/Resources/AppIcon.icns').stat().st_size > 1000
    run('codesign', '--verify', '--deep', '--strict', '-R', REQUIREMENT, str(bundle))
    run('xcrun', 'stapler', 'validate', str(bundle))
    run('spctl', '--assess', '--type', 'execute', str(bundle))
    for binary in list((bundle / 'Contents/MacOS').iterdir()) + [bundle / 'Contents/Helpers/RemoventTray.app/Contents/MacOS/RemoventTray']:
        assert run('lipo', '-archs', str(binary)).strip() == b'arm64', binary
        dependencies = run('otool', '-L', str(binary)).decode()
        assert '/opt/homebrew/' not in dependencies and '/usr/local/' not in dependencies, dependencies


def main():
    zip_path = DIST / f'Removent-{VERSION}-macos-arm64.zip'
    dmg = DIST / f'Removent-{VERSION}-macos-arm64.dmg'
    manifest = json.loads((DIST / 'latest.json').read_text())
    assert manifest['version'] == VERSION
    assert manifest['url'] == f'https://github.com/backrunner/removent/releases/download/v{VERSION}/{zip_path.name}'
    assert manifest['sha256'] == hashlib.file_digest(zip_path.open('rb'), 'sha256').hexdigest()
    with tempfile.TemporaryDirectory() as td:
        public = pathlib.Path(td) / 'key.pem'
        # Only the embedded public key is used; verification does not need CI secrets.
        import re
        source = (ROOT / 'crates/app/src/updater.rs').read_text()
        key = bytes.fromhex(re.search(r'RELEASE_PUBLIC_KEY_HEX: &str =\s*"([0-9a-f]{64})"', source).group(1))
        der = pathlib.Path(td) / 'key.der'
        der.write_bytes(bytes.fromhex('302a300506032b6570032100') + key)
        run(OPENSSL, 'pkey', '-pubin', '-inform', 'DER', '-in', str(der), '-out', str(public))
        assert verify_payload(signing_payload(manifest), manifest['signature'], str(public))
        with zipfile.ZipFile(zip_path) as archive:
            assert all(n.startswith('Removent.app/') or n.startswith('__MACOSX/') for n in archive.namelist())
        run('ditto', '-x', '-k', str(zip_path), td)
        verify_bundle(pathlib.Path(td) / 'Removent.app')
    run('hdiutil', 'verify', str(dmg))
    run('codesign', '--verify', '--strict', str(dmg))
    run('xcrun', 'stapler', 'validate', str(dmg))
    run('spctl', '--assess', '--type', 'open', '--context', 'context:primary-signature', str(dmg))
    subprocess.run(['shasum', '-a', '256', '-c', 'SHA256SUMS'], cwd=DIST, check=True)
    print('Release verified:', VERSION)

if __name__ == '__main__':
    main()
