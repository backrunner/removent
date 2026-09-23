#!/usr/bin/env python3
"""One source of truth for artifact, SemVer and Apple bundle versions."""
import os
import pathlib
import sys
import tomllib
from release_version import release_metadata

ROOT = pathlib.Path(__file__).resolve().parent.parent
VERSION = tomllib.loads((ROOT / 'Cargo.toml').read_text())['workspace']['package']['version']
# CFBundleVersion uses Apple's numeric build format; the full application
# version is preserved as RemoventReleaseVersion in Info.plist.
BUILD = os.environ.get('RELEASE_BUILD_NUMBER', '1')
try:
    VALUES = release_metadata(VERSION, BUILD)
except ValueError as error:
    raise SystemExit(str(error)) from error
BASE = VALUES['base']
if __name__ == '__main__':
    print(VALUES[sys.argv[1] if len(sys.argv) > 1 else 'version'])
