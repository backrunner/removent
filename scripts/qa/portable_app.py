#!/usr/bin/env python3
"""Verify a visible GPUI window renders and completes its isolated startup probe."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time

from PIL import ImageChops, ImageGrab


def windows_controls(pid, screenshot, evidence):
    """Check visible GPUI caption glyphs and real native minimize/maximize clicks."""
    import ctypes as c
    from ctypes import wintypes as w
    user = c.WinDLL("user32", use_last_error=True)
    callback = c.WINFUNCTYPE(w.BOOL, w.HWND, w.LPARAM)
    user.EnumWindows.argtypes = [callback, w.LPARAM]
    user.GetWindowThreadProcessId.argtypes = [w.HWND, c.POINTER(w.DWORD)]
    user.IsWindowVisible.argtypes = [w.HWND]
    user.GetClientRect.argtypes = [w.HWND, c.POINTER(w.RECT)]
    user.ClientToScreen.argtypes = [w.HWND, c.POINTER(w.POINT)]
    user.GetDpiForWindow.argtypes = [w.HWND]
    user.SendMessageW.argtypes = [w.HWND, w.UINT, w.WPARAM, w.LPARAM]
    user.SendMessageW.restype = w.LPARAM
    user.ShowWindow.argtypes = [w.HWND, c.c_int]
    user.IsIconic.argtypes = [w.HWND]
    user.IsZoomed.argtypes = [w.HWND]
    windows = []
    @callback
    def visit(hwnd, _):
        owner = w.DWORD()
        rect = w.RECT()
        user.GetWindowThreadProcessId(hwnd, c.byref(owner))
        user.GetClientRect(hwnd, c.byref(rect))
        if owner.value == pid and user.IsWindowVisible(hwnd) and rect.right > 500:
            windows.append(hwnd)
        return True
    user.EnumWindows(visit, 0)
    assert windows, "Native desktop window missing"
    hwnd = windows[0]
    rect, origin = w.RECT(), w.POINT()
    user.GetClientRect(hwnd, c.byref(rect))
    user.ClientToScreen(hwnd, c.byref(origin))
    # gpui-component 0.7 uses three 34-DIP caption buttons at the right edge.
    scale = user.GetDpiForWindow(hwnd) / 96
    controls = {}
    for index, (name, hit) in enumerate((("close", 20), ("maximize", 9), ("minimize", 8))):
        x = round(origin.x + rect.right - (index + 0.5) * 34 * scale)
        y = round(origin.y + 17 * scale)
        assert 10 < x < screenshot.width - 10 and 10 < y < screenshot.height - 10, "Caption control is off-screen"
        deadline = time.monotonic() + 2
        while True:
            low, high = screenshot.crop((x - 10, y - 10, x + 10, y + 10)).convert("L").getextrema()
            if high - low > 80:
                break
            assert time.monotonic() < deadline, f"Caption glyph is missing: {name}"
            time.sleep(0.1)
            screenshot = ImageGrab.grab().convert("RGB")
        user.SetCursorPos(x, y)
        time.sleep(0.12)  # GPUI resolves the current pointer's control area.
        observed = user.SendMessageW(hwnd, 0x84, 0, (y << 16) | (x & 0xffff))
        assert observed == hit, f"Native caption hit test for {name}: {observed}"
        controls[name] = [x, y]
    screenshot.save(evidence / "desktop.png")
    def wait_for(check, message):
        deadline = time.monotonic() + 2
        while not check():
            assert time.monotonic() < deadline, message
            time.sleep(0.05)
    for name, check in (("minimize", user.IsIconic), ("maximize", user.IsZoomed)):
        user.SetCursorPos(*controls[name])
        time.sleep(0.12)
        user.mouse_event(0x2, 0, 0, 0, 0)
        user.mouse_event(0x4, 0, 0, 0, 0)
        wait_for(lambda: check(hwnd), f"Native {name} click failed")
        user.ShowWindow(hwnd, 9)  # Restore without sending a synthetic caption action.
        wait_for(lambda: not check(hwnd), f"Window did not restore after {name}")
        time.sleep(0.2)
    (evidence / "window-controls.json").write_text(json.dumps(controls))
    print("PASS: visible caption glyphs, native hit tests and minimize/maximize clicks")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--bin-dir", type=Path, default=Path("target/release"))
    parser.add_argument("--evidence-dir", type=Path, default=Path("target/qa-evidence"))
    args = parser.parse_args()
    if os.name == "nt":
        import ctypes
        ctypes.windll.shcore.SetProcessDpiAwareness(2)
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
                if os.name == "nt":
                    windows_controls(child.pid, after, args.evidence_dir)
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
