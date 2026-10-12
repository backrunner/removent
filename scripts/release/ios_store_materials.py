#!/usr/bin/env python3
"""Validate public iOS templates and assemble an ignored, private review package."""
import argparse
import hashlib
import json
import os
import plistlib
import re
import shutil
import struct
import subprocess
import tempfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PUBLIC = ROOT / "docs/app-store/ios"
OUTPUT = ROOT / "dist/app-store-ios"
KINDS = {"connections", "connection-form", "remote-screen", "landscape-keyboard", "settings"}
LOCALES = {"en-US", "zh-Hans"}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def png_size(path):
    data = path.read_bytes()[:24]
    require(data[:8] == bytes([137, 80, 78, 71, 13, 10, 26, 10]), "Screenshot is not PNG")
    return struct.unpack(">II", data[16:24])


def load_private(path):
    require(path.is_file(), "Private review configuration is missing")
    result = subprocess.run(["git", "check-ignore", "--quiet", str(path)], cwd=ROOT)
    require(result.returncode == 0, "Private configuration must be Git ignored")
    require(path.stat().st_mode & 0o077 == 0, "Private configuration must use mode 0600")
    data = json.loads(path.read_text())
    contact = data.get("review_contact", {})
    require(all(isinstance(contact.get(k), str) and contact[k].strip() for k in ["first_name", "last_name", "email", "phone"]), "Private review contact is incomplete")
    require(re.fullmatch(r"[^\s@]+@[^\s@]+[.][^\s@]+", contact["email"]), "Private review email is invalid")
    return data


def verify_public(private=None):
    meta = json.loads((PUBLIC / "metadata.json").read_text())
    info = plistlib.loads((ROOT / "apps/mobile/Info.plist").read_bytes())
    require((meta["version"], meta["build"]) == (info["CFBundleShortVersionString"], info["CFBundleVersion"]), "Materials version differs from the app")
    require(meta["operator"] == "Alkinum" and meta["copyright"] == "2026 Alkinum", "Public operator must be Alkinum")
    require(set(meta["localizations"]) == LOCALES, "Both store localizations are required")
    for locale, fields in meta["localizations"].items():
        for key, maximum in [("name", 30), ("subtitle", 30), ("keywords", 100), ("promotional_text", 170)]:
            require(0 < len(fields[key]) <= maximum, "Store field exceeds its limit: " + locale + "/" + key)
        description = (PUBLIC / locale / "description.txt").read_text().strip()
        require(0 < len(description) <= 4000, "Description exceeds its limit")
        for key in ["support_url", "privacy_url"]:
            require(fields[key].startswith("https://removent.pwp.sh/"), "Unexpected public URL")
    require(len((PUBLIC / "review-notes.en.txt").read_bytes()) <= 4000, "Review notes exceed 4000 UTF-8 bytes")
    files = subprocess.check_output(["git", "ls-files", "--cached", "--others", "--exclude-standard", "docs/app-store/ios", "apps/mobile", "apps/website", "scripts/release/ios_store_materials.py"], cwd=ROOT, text=True).splitlines()
    secrets = []
    if private:
        contact = private["review_contact"]
        secrets = [contact[k] for k in ["first_name", "phone"]]
        if contact["email"] != "dev@backrunner.top":
            secrets.append(contact["email"])
    for name in files:
        file = ROOT / name
        if not file.is_file() or file.suffix.lower() in {".png", ".jpg", ".woff2", ".ico"}:
            continue
        text = file.read_text(errors="replace")
        require(not any(value and value.lower() in text.lower() for value in secrets), "Private review data in a public file: " + name)
    manifest = plistlib.loads((ROOT / "apps/mobile/Resources/PrivacyInfo.xcprivacy").read_bytes())
    require(manifest.get("NSPrivacyTracking") is False, "Unexpected tracking declaration")
    return meta


def verify_visuals():
    path = OUTPUT / "screenshot-manifest.json"
    require(path.is_file(), "Screenshot manifest is missing")
    manifest = json.loads(path.read_text())
    meta = json.loads((PUBLIC / "metadata.json").read_text())
    require((manifest.get("version"), manifest.get("build"), manifest.get("bundle_id")) == (meta["version"], meta["build"], meta["bundle_id"]), "Visual provenance differs from the app")
    for name, digest in manifest.get("source_file_hashes", {}).items():
        source = Path(name)
        require(not source.is_absolute() and ".." not in source.parts, "Unsafe source provenance path")
        require(hashlib.sha256((ROOT / source).read_bytes()).hexdigest() == digest, "App source changed since capture: " + name)
    seen = set()
    require(len(manifest["screenshots"]) == 20, "Exactly twenty screenshots are required")
    for item in manifest["screenshots"]:
        relative = Path(item["file"])
        require(not relative.is_absolute() and ".." not in relative.parts, "Unsafe screenshot path")
        file = OUTPUT / relative
        dimensions = png_size(file)
        require(item["device"] in {"iPhone", "iPad"}, "Unknown screenshot device")
        expected = (1206, 2622) if item["device"] == "iPhone" else (2064, 2752)
        if item["kind"] == "landscape-keyboard":
            expected = tuple(reversed(expected))
        require(dimensions == expected, "Screenshot has the wrong size: " + item["file"])
        require(hashlib.sha256(file.read_bytes()).hexdigest() == item["sha256"], "Screenshot hash changed")
        require(item["locale"] in LOCALES and item["kind"] in KINDS, "Unknown screenshot classification")
        classification = (item["locale"], item["device"], item["kind"])
        require(classification not in seen, "Duplicate screenshot classification")
        seen.add(classification)
    require(seen == {(l, d, k) for l in LOCALES for d in ["iPhone", "iPad"] for k in KINDS}, "Twenty required screenshots are incomplete")
    video = OUTPUT / "review-demo.mp4"
    require(video.is_file() and video.stat().st_size > 0, "Review demonstration video is missing")
    require(hashlib.sha256(video.read_bytes()).hexdigest() == manifest["video"]["sha256"], "Review video hash changed")
    return manifest


def package(private_path):
    private = load_private(private_path)
    meta = verify_public(private)
    verify_visuals()
    require((OUTPUT / "readiness.json").is_file(), "Readiness evidence is missing")
    # Fresh staging prevents files from an earlier contact/package being retained.
    with tempfile.TemporaryDirectory(prefix="private-package-", dir=OUTPUT) as temporary:
        assemble(Path(temporary), private, meta)


def assemble(staging, private, meta):
    shutil.copytree(PUBLIC, staging / "materials")
    for name in ["screenshots", "screenshot-manifest.json", "review-demo.mp4", "readiness.json", "onfire/production-ticket-verification.json", "public-url-checks.json", "production-api-checks.json", "visual-privacy-check.json"]:
        source = OUTPUT / name
        if source.is_dir():
            shutil.copytree(source, staging / name)
        elif source.is_file():
            (staging / name).parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, staging / name)
    payload = {**meta, "review_contact": private["review_contact"], "seller_legal_name": private.get("seller_legal_name"), "review_notes": (PUBLIC / "review-notes.en.txt").read_text()}
    contact_file = staging / "connect-payload.private.json"
    contact_file.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n")
    os.chmod(contact_file, 0o600)
    inventory = {str(p.relative_to(staging)): hashlib.sha256(p.read_bytes()).hexdigest() for p in staging.rglob("*") if p.is_file()}
    (staging / "package-manifest.json").write_text(json.dumps(inventory, indent=2) + "\n")
    archive = OUTPUT / ("Removent-iOS-" + meta["version"] + "-" + meta["build"] + "-private.zip")
    fd = os.open(archive, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "wb") as stream, zipfile.ZipFile(stream, "w", zipfile.ZIP_DEFLATED) as z:
        for file in sorted(staging.rglob("*")):
            if file.is_file():
                z.write(file, str(file.relative_to(staging)))
    os.chmod(archive, 0o600)
    print("Private package prepared in dist/app-store-ios/ (mode 0600).")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--templates-only", action="store_true")
    parser.add_argument("--package", action="store_true")
    parser.add_argument("--private-config", type=Path, default=ROOT / "userdata/release/ios-review.private.json")
    args = parser.parse_args()
    try:
        if args.package:
            package(args.private_config.resolve())
        else:
            private = load_private(args.private_config.resolve()) if args.private_config.exists() else None
            verify_public(private)
            if not args.templates_only:
                verify_visuals()
            print("iOS store materials validation passed.")
    except (ValueError, KeyError, OSError, json.JSONDecodeError) as error:
        parser.exit(1, "Materials validation failed: " + str(error) + "\n")


if __name__ == "__main__":
    main()
