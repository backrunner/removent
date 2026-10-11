#!/usr/bin/env python3
"""Verify the identity of every shipped executable, not just the outer bundle."""
import argparse
from pathlib import Path
import re
import subprocess

TEAM = 'PB8H83VL3Z'
COMPONENTS = (
    ('', 'com.alkinum.removent'),
    ('Contents/MacOS/removent', 'com.alkinum.removent.desktop'),
    ('Contents/MacOS/removent-cli', 'com.alkinum.removent.cli'),
    ('Contents/Helpers/RemoventHost.app', 'com.alkinum.removent.host'),
    ('Contents/Helpers/RemoventTray.app', 'com.alkinum.removent.tray'),
    ('Contents/Helpers/RemoventSync.app', 'com.alkinum.removent.sync'),
)


def verify(bundle, *, release=False, team=TEAM):
    signer = None
    for relative, identifier in COMPONENTS:
        component = Path(bundle) / relative
        metadata = subprocess.run(['codesign', '-d', '-vv', str(component)],
                                  check=True, capture_output=True, text=True).stderr
        authorities = re.findall(r'^Authority=(.+)$', metadata, re.MULTILINE)
        teams = re.findall(r'^TeamIdentifier=(.+)$', metadata, re.MULTILINE)
        if not authorities or len(teams) != 1 or teams[0] == 'not set':
            raise RuntimeError(f'Missing certificate identity: {component}')
        identity = (authorities[0], teams[0])
        if signer is None:
            signer = identity
        elif identity != signer:
            raise RuntimeError(f'Mixed signing identities: {component}')
        if release:
            if not identity[0].startswith('Developer ID Application: ') or identity[1] != team:
                raise RuntimeError(f'Unexpected release identity: {component}')
            if not re.search(r'^CodeDirectory .+flags=0x[0-9a-f]+\([^\n]*\bruntime\b', metadata,
                             re.MULTILINE):
                raise RuntimeError(f'Missing hardened runtime: {component}')
            if not re.search(r'^Timestamp=.+$', metadata, re.MULTILINE):
                raise RuntimeError(f'Missing secure timestamp: {component}')
        requirement = (f'=anchor apple generic and identifier "{identifier}" '
                       f'and certificate leaf[subject.OU] = "{identity[1]}"')
        if release:
            requirement += ' and certificate leaf[field.1.2.840.113635.100.6.1.13] exists'
        subprocess.run(['codesign', '--verify', '--all-architectures', '--strict',
                        '-R', requirement, str(component)], check=True)
    subprocess.run(['codesign', '--verify', '--deep', '--strict', str(bundle)], check=True)
    return signer


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('app', type=Path)
    parser.add_argument('--release', action='store_true')
    args = parser.parse_args()
    print('Verified all component signatures:', verify(args.app, release=args.release))
