#!/usr/bin/env python3
"""Record and verify the source snapshot behind a local macOS bundle."""
import argparse
import hashlib
import json
from pathlib import Path
import plistlib
import subprocess
from datetime import datetime, timezone

ROOT = Path(__file__).resolve().parents[2]
INPUTS = ('Cargo.toml', 'Cargo.lock', '.cargo/', 'packages/', 'vendor/',
          'apps/desktop/', 'apps/daemon/', 'apps/cli/', 'apps/tray/',
          'apps/installer/', 'apps/cloud-sync-helper/', 'assets/', 'scripts/build/')


def source_digest():
    paths = subprocess.check_output(
        ['git', 'ls-files', '-z', '--cached', '--others', '--exclude-standard'], cwd=ROOT)
    digest = hashlib.sha256()
    for name in sorted(set(paths.decode().split('\0'))):
        if not name or not name.startswith(INPUTS):
            continue
        path = ROOT / name
        digest.update(name.encode() + b'\0')
        digest.update(path.read_bytes() if path.is_file() else b'<deleted>')
        digest.update(b'\0')
    return digest.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['snapshot', 'write', 'verify'])
    parser.add_argument('--app', type=Path)
    parser.add_argument('--expected')
    args = parser.parse_args()
    digest = source_digest()
    if args.action == 'snapshot':
        print(digest)
        return
    if args.app is None:
        parser.error('--app is required')
    stamp = args.app / 'Contents/Resources/build-info.json'
    if args.action == 'verify':
        info = json.loads(stamp.read_text())
        if info['source_sha256'] != digest:
            raise SystemExit('Bundle is older than the current source. Rebuild with scripts/build/package.sh.')
        print(json.dumps(info, ensure_ascii=False))
        return
    if args.expected != digest:
        raise SystemExit('Source changed during the build. Rebuild before installing.')
    revision = subprocess.check_output(['git', 'rev-parse', '--short', 'HEAD'], cwd=ROOT, text=True).strip()
    dirty = bool(subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT))
    label = revision + ('-local' if dirty else '')
    stamp.write_text(json.dumps({'revision': label, 'source_sha256': digest,
                                'built_at': datetime.now(timezone.utc).isoformat()}, indent=2) + '\n')
    plist = args.app / 'Contents/Info.plist'
    info = plistlib.loads(plist.read_bytes())
    info['RemoventBuildRevision'] = label
    info['RemoventSourceSHA256'] = digest
    plist.write_bytes(plistlib.dumps(info))


if __name__ == '__main__':
    main()
