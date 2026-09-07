#!/usr/bin/env python3
"""Generate latest.json (auto-update manifest, .agents/release.md §3).

Usage: gen_latest.py <version> <zip-url> [--file PATH] [--notes TEXT]
                     [--min-proto N] [--key PEM] [--self-test]

Computes the artifact's sha256 and writes latest.json to stdout. When an
Ed25519 private key is available (--key or UPDATE_SIGNING_KEY_FILE), the
payload "{version}\n{url}\n{sha256}\n{min_compatible_proto}" (UTF-8, no
trailing newline) is signed via `openssl pkeyutl -sign -rawin` and the hex
signature is stored in the `signature` field. The release CLI requires a matching signing key and refuses unsigned manifests.
"""
import argparse
import hashlib
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
import urllib.request


def openssl_binary() -> str:
    """macOS ships LibreSSL, whose pkeyutl does not support our Ed25519 flow."""
    configured = os.environ.get("REMOVENT_OPENSSL")
    candidates = [configured] if configured else [
        "/opt/homebrew/opt/openssl@3/bin/openssl",
        "/usr/local/opt/openssl@3/bin/openssl",
        shutil.which("openssl"),
    ]
    for candidate in candidates:
        if not candidate or not os.path.isfile(candidate):
            continue
        result = subprocess.run([candidate, "version"], capture_output=True, text=True)
        version = re.match(r"OpenSSL (\d+)\.", result.stdout)
        if result.returncode == 0 and version and int(version[1]) >= 3:
            return candidate
    raise RuntimeError("OpenSSL 3+ is required: brew install openssl@3, or set REMOVENT_OPENSSL to its executable")


OPENSSL = openssl_binary()


def sha256_of(path_or_url: str) -> str:
    h = hashlib.sha256()
    if path_or_url.startswith("http"):
        with urllib.request.urlopen(path_or_url) as f:
            for chunk in iter(lambda: f.read(1 << 20), b""):
                h.update(chunk)
    else:
        with open(path_or_url, "rb") as f:
            for chunk in iter(lambda: f.read(1 << 20), b""):
                h.update(chunk)
    return h.hexdigest()


def sign_payload(payload: bytes, key_path: str) -> str:
    """Ed25519-sign payload with openssl; return the hex-encoded signature."""
    # pkeyutl -rawin is a one-shot operation and refuses stdin, so the
    # payload goes through a temp file.
    with tempfile.NamedTemporaryFile(suffix=".msg") as mf:
        mf.write(payload)
        mf.flush()
        out = subprocess.run(
            [OPENSSL, "pkeyutl", "-sign", "-rawin",
             "-inkey", key_path, "-in", mf.name],
            capture_output=True,
            check=True,
        )
    return out.stdout.hex()


def verify_payload(payload: bytes, sig_hex: str, pub_path: str) -> bool:
    """Verify a hex signature with openssl; True iff it verifies."""
    with tempfile.NamedTemporaryFile(suffix=".msg") as mf, \
            tempfile.NamedTemporaryFile(suffix=".sig") as sf:
        mf.write(payload)
        mf.flush()
        sf.write(bytes.fromhex(sig_hex))
        sf.flush()
        out = subprocess.run(
            [OPENSSL, "pkeyutl", "-verify", "-rawin", "-pubin",
             "-inkey", pub_path, "-sigfile", sf.name, "-in", mf.name],
            capture_output=True,
        )
    return out.returncode == 0


def signing_payload(m: dict) -> bytes:
    """The exact bytes the client verifier reconstructs (see release.md §3)."""
    return "{}\n{}\n{}\n{}".format(
        m["version"], m["url"], m["sha256"], m["min_compatible_proto"]
    ).encode("utf-8")


def build_manifest(version: str, url: str, notes: str, min_proto: int,
                   file_path: str | None, key_path: str | None) -> dict:
    digest = sha256_of(file_path or url)
    manifest = {
        "version": version,
        "url": url,
        "sha256": digest,
        "notes": notes,
        "pub_date": __import__("datetime").datetime.now(
            __import__("datetime").timezone.utc
        ).isoformat(),
        "min_compatible_proto": min_proto,
        "signature": "",
    }
    if key_path:
        manifest["signature"] = sign_payload(signing_payload(manifest), key_path)
    else:
        print("warning: no signing key (--key / UPDATE_SIGNING_KEY_FILE); "
              "leaving signature empty (development only)", file=sys.stderr)
    return manifest


def self_test() -> int:
    """Sign with a throwaway key, verify, and check tampering is detected."""
    failures = 0

    def check(name: str, ok: bool) -> None:
        nonlocal failures
        print(("PASS" if ok else "FAIL") + ": " + name)
        if not ok:
            failures += 1

    with tempfile.TemporaryDirectory() as td:
        key = os.path.join(td, "key.pem")
        pub = os.path.join(td, "pub.pem")
        subprocess.run([OPENSSL, "genpkey", "-algorithm", "ed25519",
                        "-out", key], capture_output=True, check=True)
        subprocess.run([OPENSSL, "pkey", "-in", key, "-pubout",
                        "-out", pub], capture_output=True, check=True)

        artifact = os.path.join(td, "artifact.zip")
        with open(artifact, "wb") as f:
            f.write(b"removent self-test artifact\n")

        m = build_manifest("9.9.9", "https://example.test/artifact.zip",
                           "self-test", 1, artifact, key)
        check("signature is non-empty hex", bool(m["signature"])
              and all(c in "0123456789abcdef" for c in m["signature"]))
        check("signature verifies",
              verify_payload(signing_payload(m), m["signature"], pub))

        for field, bad in [("version", "9.9.8"),
                           ("url", "https://evil.test/artifact.zip"),
                           ("sha256", "0" * 64),
                           ("min_compatible_proto", 2)]:
            tampered = dict(m)
            tampered[field] = bad
            check("tampered %s rejected" % field,
                  not verify_payload(signing_payload(tampered),
                                     m["signature"], pub))

        # No key -> empty signature + warning, manifest still produced.
        m2 = build_manifest("9.9.9", "https://example.test/a.zip",
                            "", 1, artifact, None)
        check("keyless manifest has empty signature", m2["signature"] == "")

    print("self-test: %s" % ("OK" if failures == 0 else "%d FAILURES" % failures))
    return 0 if failures == 0 else 1


def check_release_key(key_path: str) -> None:
    public = subprocess.run([OPENSSL, 'pkey', '-in', key_path, '-pubout', '-outform', 'DER'],
                            capture_output=True, check=True).stdout
    # RFC 8410 SubjectPublicKeyInfo header for Ed25519, then exactly 32 bytes.
    if len(public) != 44 or public[:12].hex() != '302a300506032b6570032100':
        raise ValueError('Update key must be Ed25519')
    source = (pathlib.Path(__file__).resolve().parent.parent / 'crates/app/src/updater.rs').read_text()
    expected = re.search(r'RELEASE_PUBLIC_KEY_HEX: &str =\s*"([0-9a-f]{64})"', source).group(1)
    if public[12:].hex() != expected:
        raise ValueError('Update signing key does not match the public key compiled into the app')


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("version", nargs="?")
    ap.add_argument("zip_url", nargs="?")
    ap.add_argument("--notes", default="")
    ap.add_argument("--notes-file")
    ap.add_argument("--check-key", metavar="PEM")
    ap.add_argument("--file", default=None,
                    help="local artifact path (for sha256 before upload)")
    ap.add_argument("--min-proto", type=int, default=1,
                    help="min compatible PROTO_VERSION (default 1)")
    ap.add_argument("--key", default=os.environ.get("UPDATE_SIGNING_KEY_FILE"),
                    help="Ed25519 private key PEM (or UPDATE_SIGNING_KEY_FILE)")
    ap.add_argument("--self-test", action="store_true",
                    help="sign+verify with a throwaway key and exit")
    args = ap.parse_args()

    if args.self_test:
        return self_test()
    if args.check_key:
        check_release_key(args.check_key)
        print("Update signing key matches the compiled public key")
        return 0

    if not args.version or not args.zip_url:
        ap.error("version and zip_url are required (unless --self-test)")

    key = args.key
    if key and not os.path.isfile(key):
        print("error: signing key not found: %s" % key, file=sys.stderr)
        return 1

    if not key:
        ap.error("a signing key is required for release manifests")
    check_release_key(key)
    notes = pathlib.Path(args.notes_file).read_text() if args.notes_file else args.notes
    manifest = build_manifest(args.version, args.zip_url, notes,
                              args.min_proto, args.file, key)
    json.dump(manifest, sys.stdout, ensure_ascii=False, indent=2)
    print()
    return 0


if __name__ == "__main__":
    sys.exit(main())
