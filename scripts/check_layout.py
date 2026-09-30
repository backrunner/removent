#!/usr/bin/env python3
"""Enforce workspace ownership, dependency direction and source size budgets."""
from pathlib import Path
import re
import tomllib

ROOT = Path(__file__).resolve().parent.parent


def main():
    errors = []
    workspace = tomllib.loads((ROOT / 'Cargo.toml').read_text())['workspace']
    members = set(workspace['members'])
    manifests = {str(p.parent.relative_to(ROOT))
                 for scope in ('apps', 'packages')
                 for p in (ROOT / scope).glob('*/Cargo.toml')}
    if members != manifests:
        errors.append(f'Workspace membership differs from manifests: {members ^ manifests}')
    for member in sorted(members):
        manifest = tomllib.loads((ROOT / member / 'Cargo.toml').read_text())
        if member.startswith('packages/'):
            dependencies = [manifest.get('dependencies', {}), manifest.get('build-dependencies', {})]
            for target in manifest.get('target', {}).values():
                dependencies.extend([target.get('dependencies', {}), target.get('build-dependencies', {})])
            for table in dependencies:
                for name, dependency in table.items():
                    if not isinstance(dependency, dict):
                        continue
                    base = ROOT / member
                    if dependency.get('workspace'):
                        dependency = workspace['dependencies'].get(name, {})
                        base = ROOT
                    if 'path' in dependency:
                        path = (base / dependency['path']).resolve()
                        if path.is_relative_to(ROOT / 'apps'):
                            errors.append(f'{member}: shared dependency {name} points into apps/')
        rust_sources = list((ROOT / member / 'src').rglob('*.rs'))
        rust_sources.extend((ROOT / member / 'tests').rglob('*.rs'))
        for path in rust_sources:
            test = path.name == 'tests.rs' or 'tests' in path.parts
            limit = 1000 if test else 650
            lines = len(path.read_text().splitlines())
            if lines > limit:
                errors.append(f'{path.relative_to(ROOT)}: {lines} lines exceed {limit}; split by responsibility')
    for directory in ('apps/tray/Sources', 'apps/mobile/Sources',
                      'apps/cloud-sync-helper/SyncHelper', 'apps/installer',
                      'packages/apple-cloud-sync/Sources'):
        for path in (ROOT / directory).rglob('*.swift'):
            lines = len(path.read_text().splitlines())
            if lines > 650:
                errors.append(f'{path.relative_to(ROOT)}: {lines} lines exceed 650; split by responsibility')
    public_keys = []
    for source, constant in (('apps/desktop/src/updater/manifest.rs', 'RELEASE_PUBLIC_KEY_HEX'),
                             ('apps/relay/src/updater/mod.rs', 'PUBLIC_KEY')):
        match = re.search(rf'{constant}: &str =\s*"([0-9a-f]{{64}})"',
                          (ROOT / source).read_text())
        if not match:
            errors.append(f'{source}: release public key declaration is missing')
        else:
            public_keys.append(match[1])
    if len(set(public_keys)) > 1:
        errors.append('Desktop and relay release verification keys differ')
    for legacy in ('crates', 'website', 'tray', 'mobile', 'apple', 'deploy', '.agents'):
        if (ROOT / legacy).exists():
            errors.append(f'Legacy directory remains: {legacy}/')
    if errors:
        raise SystemExit('\n'.join(errors))
    print(f'Layout OK: {len(members)} Rust packages; shared dependencies stay outside apps/')


if __name__ == '__main__':
    main()
