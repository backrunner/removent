import copy
import datetime
import unittest
from configure_cloud_sync import BUNDLE_ID, CONTAINER, profile_entitlements


class CloudSyncProfileTests(unittest.TestCase):
    def setUp(self):
        self.profile = {
            'TeamIdentifier': ['TESTTEAM'], 'ProvisionsAllDevices': True,
            'ExpirationDate': datetime.datetime.now() + datetime.timedelta(days=30),
            'Entitlements': {
                'com.apple.application-identifier': f'TESTTEAM.{BUNDLE_ID}',
                'com.apple.developer.icloud-container-identifiers': [CONTAINER],
                'com.apple.developer.icloud-services': ['CloudKit'],
                'com.apple.developer.icloud-container-environment': ['Development', 'Production'],
                'com.apple.developer.aps-environment': 'production',
            },
        }

    def test_production_profile_emits_only_required_entitlements(self):
        result = profile_entitlements(self.profile, 'TESTTEAM', 'Production')
        self.assertEqual(result['com.apple.developer.icloud-container-environment'], 'Production')
        self.assertNotIn('keychain-access-groups', result)
        self.assertNotIn('com.apple.security.app-sandbox', result)

    def test_wrong_team_container_environment_and_expiry_fail_closed(self):
        invalid = []
        for key, value in [('com.apple.application-identifier', 'TESTTEAM.other'),
                           ('com.apple.developer.icloud-container-identifiers', ['iCloud.other']),
                           ('com.apple.developer.icloud-container-environment', ['Development']),
                           ('com.apple.developer.aps-environment', 'development')]:
            profile = copy.deepcopy(self.profile); profile['Entitlements'][key] = value; invalid.append(profile)
        expired = copy.deepcopy(self.profile); expired['ExpirationDate'] = datetime.datetime(2020, 1, 1); invalid.append(expired)
        wrong_team = copy.deepcopy(self.profile); wrong_team['TeamIdentifier'] = ['OTHER']; invalid.append(wrong_team)
        development = copy.deepcopy(self.profile); development['ProvisionsAllDevices'] = False; invalid.append(development)
        for profile in invalid:
            with self.assertRaises(AssertionError):
                profile_entitlements(profile, 'TESTTEAM', 'Production')

    def test_development_profile_is_kept_out_of_production(self):
        self.profile['ProvisionsAllDevices'] = False
        self.profile['Entitlements']['com.apple.developer.aps-environment'] = 'development'
        result = profile_entitlements(self.profile, 'TESTTEAM', 'Development')
        self.assertEqual(result['com.apple.developer.icloud-container-environment'], 'Development')
        with self.assertRaises(AssertionError):
            profile_entitlements(self.profile, 'TESTTEAM', 'Production')

    def test_apple_wildcard_allowlist_claims_only_cloudkit(self):
        self.profile['Entitlements']['com.apple.developer.icloud-services'] = '*'
        result = profile_entitlements(self.profile, 'TESTTEAM', 'Production')
        self.assertEqual(result['com.apple.developer.icloud-services'], ['CloudKit'])

    def test_service_substrings_and_other_services_are_rejected(self):
        for services in ['CloudKitUnexpected', ['CloudDocuments'], [], None]:
            self.profile['Entitlements']['com.apple.developer.icloud-services'] = services
            with self.assertRaises(AssertionError):
                profile_entitlements(self.profile, 'TESTTEAM', 'Production')


if __name__ == '__main__':
    unittest.main()
