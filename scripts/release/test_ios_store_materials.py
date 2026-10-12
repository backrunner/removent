import importlib.util
import json
import struct
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("store", Path(__file__).with_name("ios_store_materials.py"))
store = importlib.util.module_from_spec(spec)
spec.loader.exec_module(store)


class StorePackageTests(unittest.TestCase):
    def test_rebuild_cannot_include_stale_private_files(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            public = root / "public"; public.mkdir()
            (public / "review-notes.en.txt").write_text("Review guide")
            output = root / "output"; output.mkdir()
            (output / "readiness.json").write_text("{}")
            stale = output / "private-package"; stale.mkdir()
            (stale / "old-contact.txt").write_text("old private contact")
            private = {"review_contact": {"first_name": "Synthetic", "last_name": "Reviewer", "email": "review@example.invalid", "phone": "+10000000000"}}
            with patch.object(store, "PUBLIC", public), patch.object(store, "OUTPUT", output), patch.object(store, "load_private", return_value=private), patch.object(store, "verify_public", return_value={"version": "0.1.3", "build": "4"}), patch.object(store, "verify_visuals"):
                store.package(root / "private.json")
                archive = output / "Removent-iOS-0.1.3-4-private.zip"
                self.assertEqual(archive.stat().st_mode & 0o777, 0o600)
                with zipfile.ZipFile(archive) as bundle:
                    self.assertNotIn("old-contact.txt", bundle.namelist())
                    self.assertIn("connect-payload.private.json", bundle.namelist())
                    self.assertNotIn(b"Synthetic", bundle.read("materials/review-notes.en.txt"))
                self.assertFalse(list(output.glob("private-package-*")))

    def test_unknown_device_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            png = bytes([137,80,78,71,13,10,26,10]) + bytes(8) + struct.pack(">II",2064,2752)
            (output / "capture.png").write_bytes(png)
            item = {"file":"capture.png", "device":"unknown", "locale":"en-US", "kind":"settings"}
            (output / "screenshot-manifest.json").write_text(json.dumps({"version":"0.1.3","build":"4","bundle_id":"com.alkinum.removent.mobile","screenshots":[item]*20}))
            with patch.object(store,"OUTPUT",output), patch.object(store, "PUBLIC", Path(__file__).resolve().parents[2] / "docs/app-store/ios"), self.assertRaisesRegex(ValueError,"Unknown screenshot device"):
                store.verify_visuals()

    def test_path_traversal_is_rejected_before_read(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            (output / "screenshot-manifest.json").write_text(json.dumps({"version":"0.1.3","build":"4","bundle_id":"com.alkinum.removent.mobile","screenshots":[{"file":"../private.json"}]*20}))
            with patch.object(store,"OUTPUT",output), patch.object(store, "PUBLIC", Path(__file__).resolve().parents[2] / "docs/app-store/ios"), self.assertRaisesRegex(ValueError,"Unsafe screenshot path"):
                store.verify_visuals()


if __name__ == "__main__":
    unittest.main()
