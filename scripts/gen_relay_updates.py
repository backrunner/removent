#!/usr/bin/env python3
"""Sign platform-specific relay update manifests after all release assets exist."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import tarfile

from gen_latest import check_release_key, sign_payload
from package_relay import PLATFORMS, asset_name, verify


def signing_payload(manifest):
    return ("removent-relay-v1\n" + "\n".join(manifest[field] for field in
            ("version", "platform", "url", "sha256", "binary_sha256"))).encode("utf-8")


def generate(directory, version, key, *, check_key=True):
    tag = "v" + version
    verify(directory, tag)
    if check_key:
        check_release_key(str(key))
        source = Path(__file__).resolve().parent.parent
        app = (source / "crates/app/src/updater.rs").read_text()
        relay = (source / "crates/relay/src/updater.rs").read_text()
        app_key = re.search(r'RELEASE_PUBLIC_KEY_HEX: &str =\s*"([0-9a-f]{64})"', app)[1]
        relay_key = re.search(r'PUBLIC_KEY: &str =\s*"([0-9a-f]{64})"', relay)[1]
        if app_key != relay_key:
            raise ValueError("App and relay release verification keys must agree")
    for platform in PLATFORMS:
        asset = directory / asset_name(tag, platform)
        with tarfile.open(asset, "r:gz") as archive:
            binary = archive.extractfile("removent-relay").read()
        manifest = {
            "version": version,
            "platform": platform,
            "url": f"https://github.com/backrunner/removent/releases/download/{tag}/{asset.name}",
            "sha256": hashlib.sha256(asset.read_bytes()).hexdigest(),
            "binary_sha256": hashlib.sha256(binary).hexdigest(),
        }
        manifest["signature"] = sign_payload(signing_payload(manifest), str(key))
        (directory / f"relay-latest-{platform}.json").write_text(json.dumps(manifest, indent=2) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--directory", type=Path, default=Path("dist/relay"))
    parser.add_argument("--key", required=True, type=Path)
    args = parser.parse_args()
    generate(args.directory, args.version, args.key)
