#!/usr/bin/env python3
"""Package one native release binary, or verify the complete release asset set."""
import argparse
import hashlib
import io
import pathlib
import re
import subprocess
import struct
import sys
import tarfile
import tempfile

PLATFORMS = ("linux-x86_64", "linux-aarch64", "macos-universal")
SIGNING_REQUIREMENT = ('=anchor apple generic and certificate leaf[subject.OU] = "PB8H83VL3Z" '
                       'and certificate leaf[field.1.2.840.113635.100.6.1.13] exists')


def verify_signature(binary):
    subprocess.run(["codesign", "--verify", "--all-architectures", "--strict", "-R",
                    SIGNING_REQUIREMENT, str(binary)], check=True)


def verify_universal(binary):
    if len(binary) < 8 or binary[:4] not in (b"\xca\xfe\xba\xbe", b"\xca\xfe\xba\xbf"):
        raise ValueError("Expected a universal macOS binary")
    count = int.from_bytes(binary[4:8], "big")
    wide = binary[:4] == b"\xca\xfe\xba\xbf"
    stride = 32 if wide else 20
    if count != 2 or len(binary) < 8 + count * stride:
        raise ValueError("Universal relay requires exactly ARM64 and x86_64")
    architectures = set()
    ranges = []
    for index in range(count):
        offset = 8 + index * stride
        cpu, _ = struct.unpack_from(">II", binary, offset)
        start, size = struct.unpack_from(">QQ" if wide else ">II", binary, offset + 8)
        if start < 8 + count * stride or size < 32 or start + size > len(binary):
            raise ValueError("Invalid universal slice")
        if any(start < other_end and start + size > other_start for other_start, other_end in ranges):
            raise ValueError("Overlapping universal slices")
        ranges.append((start, start + size))
        if binary[start:start + 4] != b"\xcf\xfa\xed\xfe" or int.from_bytes(binary[start + 4:start + 8], "little") != cpu:
            raise ValueError("Wrong Mach-O slice architecture")
        architectures.add(cpu)
    if architectures != {0x1000007, 0x100000C}:
        raise ValueError("Universal relay requires ARM64 and x86_64 architectures")


def asset_name(version, platform):
    if not re.fullmatch(r"v\d+\.\d+\.\d+", version):
        raise ValueError("Expected a stable version tag")
    return f"removent-relay-{version}-{platform}.tar.gz"


def verify(directory, version, platforms=PLATFORMS):
    for platform in platforms:
        name = asset_name(version, platform)
        artifact = directory / name
        expected = f"{hashlib.sha256(artifact.read_bytes()).hexdigest()}  {name}\n"
        if (directory / (name + ".sha256")).read_text() != expected:
            raise ValueError(f"Checksum mismatch: {name}")
        with tarfile.open(artifact, "r:gz") as archive:
            members = archive.getmembers()
            if len(members) != 1 or members[0].name != "removent-relay" or not members[0].isfile() or members[0].mode != 0o755:
                raise ValueError(f"Unexpected archive contents: {name}")
            binary = archive.extractfile(members[0]).read()
            if platform.startswith("linux"):
                machine = int.from_bytes(binary[18:20], "little")
                if binary[:6] != b"\x7fELF\x02\x01" or machine != (62 if platform == "linux-x86_64" else 183):
                    raise ValueError(f"Wrong ELF architecture: {name}")
            else:
                verify_universal(binary)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--directory", type=pathlib.Path, default=pathlib.Path("dist/relay"))
    parser.add_argument("--verify", action="store_true")
    parser.add_argument("--platform", choices=PLATFORMS)
    parser.add_argument("--binary", type=pathlib.Path)
    args = parser.parse_args()
    if args.verify:
        verify(args.directory, args.version)
        if sys.platform == "darwin":
            # Verify the actual archived CLI, not a possibly different build
            # product left in target/. Publishing runs on the macOS job.
            with tempfile.TemporaryDirectory(prefix="removent-relay-verify-") as temp:
                binary = pathlib.Path(temp) / "removent-relay"
                with tarfile.open(args.directory / asset_name(args.version, "macos-universal"), "r:gz") as archive:
                    binary.write_bytes(archive.extractfile("removent-relay").read())
                binary.chmod(0o755)
                verify_signature(binary)
        return
    if not args.platform or not args.binary:
        parser.error("Packaging requires --platform and --binary")
    output = subprocess.check_output([str(args.binary.resolve()), "--version"], text=True).strip()
    if output != f"removent-relay {args.version.removeprefix('v')}":
        raise ValueError("Binary version does not match the release tag")
    if args.platform == "macos-universal":
        verify_signature(args.binary)
    args.directory.mkdir(parents=True, exist_ok=True)
    name = asset_name(args.version, args.platform)
    artifact = args.directory / name
    payload = args.binary.read_bytes()
    member = tarfile.TarInfo("removent-relay")
    member.size, member.mode, member.mtime = len(payload), 0o755, 0
    with tarfile.open(artifact, "w:gz") as archive:
        archive.addfile(member, io.BytesIO(payload))
    (args.directory / (name + ".sha256")).write_text(f"{hashlib.sha256(artifact.read_bytes()).hexdigest()}  {name}\n")
    verify(args.directory, args.version, (args.platform,))
    print(artifact)


if __name__ == "__main__":
    main()
