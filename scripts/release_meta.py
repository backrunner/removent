#!/usr/bin/env python3
"""One source of truth for artifact, SemVer and Apple bundle versions."""
import os
import pathlib
import re
import sys
import tomllib

ROOT = pathlib.Path(__file__).resolve().parent.parent
VERSION = tomllib.loads((ROOT / 'Cargo.toml').read_text())['workspace']['package']['version']
MATCH = re.fullmatch(r'(\d+)\.(\d+)\.(\d+)(?:-beta\.([1-9]\d*))?', VERSION)
if not MATCH:
    raise SystemExit('release version must be MAJOR.MINOR.PATCH or MAJOR.MINOR.PATCH-beta.N')
BASE = '.'.join(MATCH.group(i) for i in (1, 2, 3))
# CFBundleVersion uses Apple's numeric build format; prerelease identity is
# preserved separately as RemoventReleaseVersion in Info.plist.
BUILD = os.environ.get('RELEASE_BUILD_NUMBER', '1')
if not re.fullmatch(r'[1-9][0-9]{0,3}', BUILD):
    raise SystemExit('RELEASE_BUILD_NUMBER must be a positive Apple build number (1–9999)')
VALUES = {'version': VERSION, 'base': BASE, 'build': BUILD,
          'channel': 'beta' if MATCH[4] else 'stable', 'tag': f'v{VERSION}'}
if __name__ == '__main__':
    print(VALUES[sys.argv[1] if len(sys.argv) > 1 else 'version'])
