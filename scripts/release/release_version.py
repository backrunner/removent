"""Supported public release channels and Apple-compatible bundle versions."""
import re

NUMBER = r"(?:0|[1-9][0-9]*)"
VERSION_PATTERN = rf"({NUMBER})\.({NUMBER})\.({NUMBER})(?:-beta\.([1-9][0-9]*))?"


def release_metadata(version, build="1"):
    match = re.fullmatch(VERSION_PATTERN, version)
    if not match:
        raise ValueError("release version must be MAJOR.MINOR.PATCH or MAJOR.MINOR.PATCH-beta.N")
    if not re.fullmatch(r"[1-9][0-9]{0,3}", build):
        raise ValueError("RELEASE_BUILD_NUMBER must be a positive Apple build number (1–9999)")
    return {
        "version": version,
        "base": ".".join(match.group(i) for i in (1, 2, 3)),
        "build": build,
        "channel": "beta" if match.group(4) else "stable",
        "tag": f"v{version}",
    }
