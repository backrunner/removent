import unittest

from package_relay import asset_name
from release_version import release_metadata


class ReleaseVersionTests(unittest.TestCase):
    def test_stable_and_beta_keep_full_semver_with_numeric_apple_versions(self):
        stable = release_metadata("0.1.3", "19")
        beta = release_metadata("0.1.3-beta.2", "20")
        self.assertEqual(stable["channel"], "stable")
        self.assertEqual(beta, {
            "version": "0.1.3-beta.2", "base": "0.1.3", "build": "20",
            "tag": "v0.1.3-beta.2", "channel": "beta",
        })
        self.assertEqual(asset_name(beta["tag"], "macos-universal"),
                         "removent-relay-v0.1.3-beta.2-macos-universal.tar.gz")

    def test_ambiguous_versions_and_invalid_apple_builds_are_rejected(self):
        for version in ("01.1.3", "0.1", "v0.1.3", "0.1.3-beta", "0.1.3-beta.0",
                        "0.1.3-beta.01", "0.1.3-preview.1", "0.1.3+build", "0.1.3;false"):
            with self.subTest(version=version), self.assertRaises(ValueError):
                release_metadata(version)
        for build in ("0", "01", "-1", "10000", "1beta"):
            with self.subTest(build=build), self.assertRaises(ValueError):
                release_metadata("0.1.3-beta.1", build)


if __name__ == "__main__":
    unittest.main()
