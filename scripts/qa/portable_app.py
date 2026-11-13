#!/usr/bin/env python3
"""Verify a visible GPUI window renders and completes its isolated startup probe."""
import argparse
import os
from pathlib import Path
import subprocess
import tempfile
import time

from PIL import ImageChops, ImageGrab


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--bin-dir", type=Path, default=Path("target/release"))
    parser.add_argument("--evidence-dir", type=Path, default=Path("target/qa-evidence"))
    args = parser.parse_args()
    args.evidence_dir.mkdir(parents=True, exist_ok=True)
    binary = args.bin_dir.resolve() / ("removent.exe" if os.name == "nt" else "removent")
    before = ImageGrab.grab().convert("RGB")
    with tempfile.TemporaryDirectory(prefix="removent-app-") as data:
        env = dict(os.environ, REMOVENT_DATA_DIR=data, REMOVENT_NO_TRAY="1")
        with (args.evidence_dir / "desktop.log").open("w") as log:
            child = subprocess.Popen([str(binary), "--desktop-smoke"], env=env,
                                     stdout=log, stderr=subprocess.STDOUT)
            try:
                # Software GPU discovery can take several seconds on a cold runner.
                # Observe presented pixels, rather than assuming a fixed startup time.
                deadline = time.monotonic() + 25
                while True:
                    assert child.poll() is None, "Desktop exited before the render observation"
                    after = ImageGrab.grab().convert("RGB")
                    after.save(args.evidence_dir / "desktop.png")
                    center = after.crop((after.width // 4, after.height // 4,
                                         after.width * 3 // 4, after.height * 3 // 4))
                    colors = center.getcolors(center.width * center.height)
                    difference = ImageChops.difference(before, after).convert("L")
                    changed = sum(difference.histogram()[24:])
                    if colors and len(colors) > 30 and changed > 20000:
                        break
                    assert time.monotonic() < deadline, (
                        f"Desktop did not visibly render ({len(colors or [])} colors, "
                        f"{changed} changed pixels)"
                    )
                    time.sleep(0.25)
                assert child.wait(timeout=25) == 0, "Desktop process failed"
                receipt = Path(data) / "run/update-smoke-ok"
                assert receipt.is_file() and receipt.read_text().strip(), "No first-frame completion receipt"
            finally:
                if child.poll() is None:
                    child.kill()
                    child.wait()
    print("PASS: visible GPUI window, rendered pixels, first-frame receipt and clean exit")


if __name__ == "__main__":
    main()
