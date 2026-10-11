import subprocess
import unittest
from unittest.mock import patch

import verify_signing


def metadata(*, development=False, team=verify_signing.TEAM, runtime=True, timestamp=True):
    authority = 'Apple Development' if development else 'Developer ID Application'
    return (f'Authority={authority}: Example ({team})\n'
            f'TeamIdentifier={team}\n'
            f'CodeDirectory v=20500 flags=0x10000({"runtime" if runtime else "none"})\n'
            + ('Timestamp=Oct 12, 2026\n' if timestamp else 'Signed Time=Oct 12, 2026\n'))


class SigningVerificationTests(unittest.TestCase):
    def verify_fixture(self, replacements=None, *, release=True, failure=None):
        replacements = replacements or {}

        def codesign(args, **kwargs):
            if args[1] == '-d':
                return subprocess.CompletedProcess(args, 0, '', replacements.get(args[-1], metadata()))
            if failure and args[-1] == failure and '--all-architectures' in args:
                raise subprocess.CalledProcessError(1, args)
            return subprocess.CompletedProcess(args, 0, '', '')

        with patch.object(verify_signing.subprocess, 'run', side_effect=codesign) as run:
            result = verify_signing.verify('/Removent.app', release=release)
        return result, run.call_args_list

    def test_every_component_has_its_own_identifier_team_and_developer_id_requirement(self):
        _, calls = self.verify_fixture()
        checks = [call.args[0] for call in calls if '--all-architectures' in call.args[0]]
        self.assertEqual(len(checks), len(verify_signing.COMPONENTS))
        for args, (relative, identifier) in zip(checks, verify_signing.COMPONENTS):
            requirement = args[args.index('-R') + 1]
            self.assertIn(f'identifier "{identifier}"', requirement)
            self.assertIn(verify_signing.TEAM, requirement)
            self.assertIn('1.2.840.113635.100.6.1.13', requirement)
            self.assertTrue(args[-1].endswith(relative))

    def test_development_signed_release_is_rejected(self):
        with self.assertRaisesRegex(RuntimeError, 'Unexpected release identity'):
            self.verify_fixture({'/Removent.app': metadata(development=True)})

    def test_wrong_team_is_rejected(self):
        with self.assertRaisesRegex(RuntimeError, 'Unexpected release identity'):
            self.verify_fixture({'/Removent.app': metadata(team='OTHERTEAM')})

    def test_mixed_host_certificate_is_rejected_in_local_and_release_packages(self):
        for release in (False, True):
            with self.subTest(release=release), self.assertRaisesRegex(RuntimeError, 'Mixed signing identities'):
                self.verify_fixture({'/Removent.app/Contents/Helpers/RemoventHost.app':
                                     metadata(development=True)}, release=release)

    def test_runtime_and_secure_timestamp_are_required_for_releases(self):
        for values, message in (({'runtime': False}, 'hardened runtime'),
                                ({'timestamp': False}, 'secure timestamp')):
            with self.subTest(values=values), self.assertRaisesRegex(RuntimeError, message):
                self.verify_fixture({'/Removent.app/Contents/Helpers/RemoventTray.app': metadata(**values)})

    def test_ad_hoc_signature_is_rejected(self):
        with self.assertRaisesRegex(RuntimeError, 'Missing certificate identity'):
            self.verify_fixture({'/Removent.app': 'Signature=adhoc\nTeamIdentifier=not set\n'})

    def test_helper_verification_failure_aborts_the_package(self):
        with self.assertRaises(subprocess.CalledProcessError):
            self.verify_fixture(failure='/Removent.app/Contents/Helpers/RemoventHost.app')

    def test_local_signing_accepts_a_consistent_development_identity_without_notarization_options(self):
        replacements = {'/Removent.app' + ('/' + relative if relative else ''):
                        metadata(development=True, runtime=False, timestamp=False)
                        for relative, _ in verify_signing.COMPONENTS}
        self.verify_fixture(replacements, release=False)


if __name__ == '__main__':
    unittest.main()
