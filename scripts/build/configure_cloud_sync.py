#!/usr/bin/env python3
"""Prepare/verify the signed CloudKit helper without printing profile contents."""
import argparse
import datetime
import os
import pathlib
import plistlib
import shutil
import subprocess

CONTAINER = 'iCloud.com.alkinum.removent'
BUNDLE_ID = 'com.alkinum.removent.sync'


def profile_entitlements(profile, team, environment):
    entitlements = profile['Entitlements']
    assert team in profile['TeamIdentifier'], 'CloudKit profile belongs to another team'
    assert profile['ExpirationDate'].replace(tzinfo=datetime.timezone.utc) > datetime.datetime.now(datetime.timezone.utc), 'CloudKit profile expired'
    assert entitlements['com.apple.application-identifier'] == f'{team}.{BUNDLE_ID}', 'Wrong CloudKit helper App ID'
    assert CONTAINER in entitlements['com.apple.developer.icloud-container-identifiers'], 'Shared CloudKit container is not authorized'
    assert 'CloudKit' in entitlements['com.apple.developer.icloud-services'], 'CloudKit service is not authorized'
    allowed = entitlements['com.apple.developer.icloud-container-environment']
    assert environment in (allowed if isinstance(allowed, list) else [allowed]), 'CloudKit environment not authorized'
    if environment == 'Production':
        assert profile.get('ProvisionsAllDevices'), 'Developer ID distribution profile required'
    aps = 'production' if environment == 'Production' else 'development'
    assert entitlements.get('com.apple.developer.aps-environment') == aps, 'Push notification environment mismatch'
    return {
        'com.apple.application-identifier': f'{team}.{BUNDLE_ID}',
        'com.apple.developer.team-identifier': team,
        'com.apple.developer.icloud-container-identifiers': [CONTAINER],
        'com.apple.developer.icloud-services': ['CloudKit'],
        'com.apple.developer.icloud-container-environment': environment,
        'com.apple.developer.aps-environment': aps,
    }


def decode_profile(path):
    return plistlib.loads(subprocess.check_output(['security', 'cms', '-D', '-i', str(path)], stderr=subprocess.DEVNULL))


def verify(bundle, team, environment='Production'):
    expected = profile_entitlements(decode_profile(bundle / 'Contents/embedded.provisionprofile'), team, environment)
    actual = plistlib.loads(subprocess.check_output(['codesign', '-d', '--entitlements', ':-', str(bundle)], stderr=subprocess.DEVNULL))
    for key, value in expected.items():
        assert actual.get(key) == value, f'CloudKit entitlement mismatch: {key}'
    requirement = f'=anchor apple generic and identifier "{BUNDLE_ID}" and certificate leaf[subject.OU] = "{team}"'
    if environment == 'Production':
        requirement += ' and certificate leaf[field.1.2.840.113635.100.6.1.13] exists'
    subprocess.run(['codesign', '--verify', '--strict', '-R', requirement, str(bundle)], check=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('bundle', type=pathlib.Path)
    parser.add_argument('--profile', type=pathlib.Path)
    parser.add_argument('--team', default=os.environ.get('APPLE_TEAM_ID', 'PB8H83VL3Z'))
    parser.add_argument('--environment', choices=['Development', 'Production'], default='Production')
    parser.add_argument('--verify', action='store_true')
    args = parser.parse_args()
    if args.verify:
        verify(args.bundle, args.team, args.environment)
    else:
        if not args.profile:
            parser.error('--profile is required')
        entitlements = profile_entitlements(decode_profile(args.profile), args.team, args.environment)
        shutil.copyfile(args.profile, args.bundle / 'Contents/embedded.provisionprofile')
        (args.bundle / 'Contents/embedded.provisionprofile').chmod(0o644)
        output = args.bundle.parent / 'removent-sync.entitlements'
        output.write_bytes(plistlib.dumps(entitlements))
        output.chmod(0o644)
    print('CloudKit helper provisioning validated')


if __name__ == '__main__':
    main()
