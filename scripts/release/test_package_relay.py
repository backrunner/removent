#!/usr/bin/env python3
import hashlib
import io
import json
import pathlib
import struct
import subprocess
import sys
import tarfile
import tempfile
import unittest

from package_relay import PLATFORMS, SIGNING_REQUIREMENT, asset_name, verify, verify_universal
from gen_latest import OPENSSL, verify_payload
from gen_relay_updates import generate, signing_payload


class ReleaseAssetTests(unittest.TestCase):
    @unittest.skipUnless(sys.platform == "darwin", "Requires Apple's signing tools")
    def test_signing_requirement_is_inline_and_rejects_another_signer(self):
        with tempfile.TemporaryDirectory() as temp:
            subprocess.run(["/usr/bin/csreq", "-r", SIGNING_REQUIREMENT, "-b",
                            str(pathlib.Path(temp) / "requirement")], check=True, capture_output=True)
        result = subprocess.run(["/usr/bin/codesign", "--verify", "--all-architectures", "--strict",
                                 "-R", SIGNING_REQUIREMENT, "/usr/bin/true"], capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("code failed to satisfy specified code requirement", result.stderr)

    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.directory = pathlib.Path(temp.name)
        for platform in PLATFORMS:
            self.write_archive(platform)

    def write_archive(self, platform, member_name="removent-relay", machine=None):
        payload = bytearray(64)
        if platform.startswith("linux"):
            payload[:6] = b"\x7fELF\x02\x01"
            payload[18:20] = (machine or (62 if platform == "linux-x86_64" else 183)).to_bytes(2, "little")
        else:
            payload = bytearray(8 + 40 + 64)
            struct.pack_into(">II", payload, 0, 0xCAFEBABE, 2)
            for index, cpu in enumerate([0x100000C, machine or 0x1000007]):
                offset = 48 + index * 32
                struct.pack_into(">IIIII", payload, 8 + index * 20, cpu, 0, offset, 32, 0)
                payload[offset:offset + 4] = b"\xcf\xfa\xed\xfe"
                payload[offset + 4:offset + 8] = cpu.to_bytes(4, "little")
        name = asset_name("v0.1.0", platform)
        artifact = self.directory / name
        with tarfile.open(artifact, "w:gz") as archive:
            member = tarfile.TarInfo(member_name)
            member.size, member.mode = len(payload), 0o755
            archive.addfile(member, io.BytesIO(payload))
        (self.directory / (name + ".sha256")).write_text(f"{hashlib.sha256(artifact.read_bytes()).hexdigest()}  {name}\n")

    def test_complete_set_and_missing_platform(self):
        verify(self.directory, "v0.1.0")
        (self.directory / asset_name("v0.1.0", "linux-aarch64")).unlink()
        with self.assertRaises(FileNotFoundError):
            verify(self.directory, "v0.1.0")

    def test_relay_manifests_sign_both_hashes_and_every_platform(self):
        key = self.directory / "test-key.pem"
        public = self.directory / "test-public.pem"
        subprocess.run([OPENSSL, "genpkey", "-algorithm", "ED25519", "-out", str(key)], check=True, capture_output=True)
        subprocess.run([OPENSSL, "pkey", "-in", str(key), "-pubout", "-out", str(public)], check=True, capture_output=True)
        with self.assertRaisesRegex(ValueError, "compiled into the app"):
            generate(self.directory, "0.1.0", key)
        generate(self.directory, "0.1.0", key, check_key=False)
        for platform in PLATFORMS:
            manifest = json.loads((self.directory / f"relay-latest-{platform}.json").read_text())
            self.assertEqual(manifest["platform"], platform)
            self.assertTrue(verify_payload(signing_payload(manifest), manifest["signature"], str(public)))
            with tarfile.open(self.directory / asset_name("v0.1.0", platform), "r:gz") as archive:
                binary = archive.extractfile("removent-relay").read()
            self.assertEqual(manifest["binary_sha256"], hashlib.sha256(binary).hexdigest())
            for field in ("version", "platform", "url", "sha256", "binary_sha256"):
                tampered = dict(manifest, **{field: "tampered"})
                self.assertFalse(verify_payload(signing_payload(tampered), manifest["signature"], str(public)), field)

    def test_universal_macos_requires_both_architectures(self):
        self.write_archive("macos-universal", machine=0x100000C)
        with self.assertRaisesRegex(ValueError, "architectures"):
            verify(self.directory, "v0.1.0")
        with self.assertRaises(ValueError):
            verify_universal(b"\xcf\xfa\xed\xfe" + bytes(60))

    def test_modified_archive_is_not_publishable(self):
        artifact = self.directory / asset_name("v0.1.0", "linux-x86_64")
        artifact.write_bytes(artifact.read_bytes() + b"tamper")
        with self.assertRaisesRegex(ValueError, "Checksum mismatch"):
            verify(self.directory, "v0.1.0")

    def test_wrong_architecture_and_unsafe_member_are_not_publishable(self):
        self.write_archive("linux-aarch64", machine=62)
        with self.assertRaisesRegex(ValueError, "architecture"):
            verify(self.directory, "v0.1.0")
        self.write_archive("linux-aarch64", member_name="../removent-relay")
        with self.assertRaisesRegex(ValueError, "archive contents"):
            verify(self.directory, "v0.1.0")


if __name__ == "__main__":
    unittest.main()
