from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import local_signing


DEVELOPMENT = 'A' * 40
DEVELOPER_ID = 'B' * 40
DEVELOPMENT_NAME = 'Apple Development: Example (DEVTEAM)'
DEVELOPER_ID_NAME = 'Developer ID Application: Example (RELEASETEAM)'
IDENTITIES = (f'  1) {DEVELOPMENT} "{DEVELOPMENT_NAME}"\n'
              f'  2) {DEVELOPER_ID} "{DEVELOPER_ID_NAME}"\n')


class LocalSigningTests(unittest.TestCase):
    @patch.object(local_signing.subprocess, 'check_output', return_value=IDENTITIES)
    @patch.object(local_signing, 'signing_authority', side_effect=[DEVELOPER_ID_NAME, DEVELOPMENT_NAME])
    def test_installed_app_and_host_keep_independent_certificates(self, authority, output):
        result = local_signing.configure_local_signing({}, Path('/Applications/Removent.app'))
        self.assertEqual(result['REMOVENT_LOCAL_SIGNING_IDENTITY'], DEVELOPER_ID)
        self.assertEqual(result['REMOVENT_LOCAL_HOST_SIGNING_IDENTITY'], DEVELOPMENT)
        self.assertEqual(authority.call_args.args[0],
                         Path('/Applications/Removent.app/Contents/Helpers/RemoventHost.app'))

    @patch.object(local_signing.subprocess, 'check_output', return_value=IDENTITIES)
    @patch.object(local_signing, 'signing_authority', return_value='Apple Development: Unavailable (OTHER)')
    def test_unavailable_installed_certificate_never_silently_changes_identity(self, authority, output):
        with self.assertRaisesRegex(RuntimeError, 'unavailable or ambiguous'):
            local_signing.configure_local_signing({}, Path('/Applications/Removent.app'))

    @patch.object(local_signing.subprocess, 'check_output', return_value=IDENTITIES)
    @patch.object(local_signing, 'signing_authority', return_value=DEVELOPER_ID_NAME)
    def test_explicit_host_repair_keeps_outer_app_identity(self, authority, output):
        environment = {'REMOVENT_LOCAL_HOST_SIGNING_IDENTITY': DEVELOPMENT}
        result = local_signing.configure_local_signing(environment, Path('/Applications/Removent.app'))
        self.assertEqual(result['REMOVENT_LOCAL_SIGNING_IDENTITY'], DEVELOPER_ID)
        self.assertEqual(result['REMOVENT_LOCAL_HOST_SIGNING_IDENTITY'], DEVELOPMENT)
        self.assertNotIn('REMOVENT_LOCAL_SIGNING_IDENTITY', environment)

    @patch.object(local_signing.subprocess, 'check_output', return_value=IDENTITIES)
    @patch.object(local_signing, 'signing_authority')
    def test_explicit_global_identity_can_deliberately_change_both_signatures(self, authority, output):
        result = local_signing.configure_local_signing(
            {'REMOVENT_LOCAL_SIGNING_IDENTITY': DEVELOPMENT}, Path('/Applications/Removent.app'))
        self.assertEqual(result['REMOVENT_LOCAL_HOST_SIGNING_IDENTITY'], DEVELOPMENT)
        authority.assert_not_called()

    @patch.object(local_signing.subprocess, 'check_output', return_value=IDENTITIES)
    @patch.object(local_signing, 'signing_authority', return_value=None)
    def test_first_install_prefers_unique_developer_id(self, authority, output):
        result = local_signing.configure_local_signing({}, Path('/Applications/Removent.app'))
        self.assertEqual(result['REMOVENT_LOCAL_SIGNING_IDENTITY'], DEVELOPER_ID)
        self.assertEqual(result['REMOVENT_LOCAL_HOST_SIGNING_IDENTITY'], DEVELOPER_ID)

    @patch.object(local_signing.subprocess, 'check_output')
    def test_release_signing_is_independent_of_local_certificate_selection(self, output):
        environment = {'APPLE_SIGNING_IDENTITY': 'Release identity'}
        self.assertEqual(local_signing.configure_local_signing(environment, Path('Removent.app')), environment)
        output.assert_not_called()

    def test_reads_leaf_authority_instead_of_root_certificate(self):
        with tempfile.TemporaryDirectory() as tmp:
            app = Path(tmp) / 'Removent.app'
            app.mkdir()
            with patch.object(local_signing.subprocess, 'run') as run:
                run.return_value.returncode = 0
                run.return_value.stderr = f'Authority={DEVELOPMENT_NAME}\nAuthority=Apple Root CA\n'
                self.assertEqual(local_signing.signing_authority(app), DEVELOPMENT_NAME)


if __name__ == '__main__':
    unittest.main()
