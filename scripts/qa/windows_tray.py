#!/usr/bin/env python3
"""Observe the real shell icon and exercise native Win32 tray menu/dialogs.

The shell callback message (6002) and owner class are from the locked tray-icon
0.25 dependency. Only opening the menu is synthesized; menu items receive actual
pointer input. Run exclusively in an isolated Windows CI desktop.
"""
import argparse
import ctypes as c
from ctypes import wintypes as w
import os
from pathlib import Path
import subprocess
import tempfile
import time
import tomllib

from PIL import ImageGrab

user = c.WinDLL("user32", use_last_error=True)
shell = c.WinDLL("shell32", use_last_error=True)
callback = c.WINFUNCTYPE(w.BOOL, w.HWND, w.LPARAM)
user.EnumWindows.argtypes = [callback, w.LPARAM]
user.GetWindowThreadProcessId.argtypes = [w.HWND, c.POINTER(w.DWORD)]
user.GetClassNameW.argtypes = [w.HWND, w.LPWSTR, c.c_int]
user.GetWindowTextW.argtypes = [w.HWND, w.LPWSTR, c.c_int]
user.PostMessageW.argtypes = [w.HWND, w.UINT, w.WPARAM, w.LPARAM]
user.SendMessageW.argtypes = [w.HWND, w.UINT, w.WPARAM, w.LPARAM]
user.SendMessageW.restype = w.LPARAM
user.GetMenuItemCount.argtypes = [w.HMENU]
user.GetMenuStringW.argtypes = [w.HMENU, w.UINT, w.LPWSTR, c.c_int, w.UINT]
user.GetMenuState.argtypes = [w.HMENU, w.UINT, w.UINT]
user.GetMenuItemRect.argtypes = [w.HWND, w.HMENU, w.UINT, c.POINTER(w.RECT)]
user.SetForegroundWindow.argtypes = [w.HWND]
user.IsWindowVisible.argtypes = [w.HWND]


class IconIdentifier(c.Structure):
    _fields_ = [("cbSize", w.DWORD), ("hWnd", w.HWND), ("uID", w.UINT),
                ("guidItem", c.c_ubyte * 16)]


shell.Shell_NotifyIconGetRect.argtypes = [c.POINTER(IconIdentifier), c.POINTER(w.RECT)]
shell.Shell_NotifyIconGetRect.restype = w.LONG


def wait_for(check, message, seconds=15):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = check()
        if value:
            return value
        time.sleep(0.1)
    raise AssertionError(message)


def window(pid, class_name=None, title=None, visible=False):
    found = []
    @callback
    def visit(hwnd, _):
        owner = w.DWORD()
        user.GetWindowThreadProcessId(hwnd, c.byref(owner))
        name, text = c.create_unicode_buffer(256), c.create_unicode_buffer(256)
        user.GetClassNameW(hwnd, name, 256)
        user.GetWindowTextW(hwnd, text, 256)
        if owner.value == pid and (class_name is None or name.value == class_name) and (title is None or text.value == title) and (not visible or user.IsWindowVisible(hwnd)):
            found.append(hwnd)
        return True
    user.EnumWindows(visit, 0)
    return found[0] if found else None


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--bin-dir", type=Path, default=Path("target/release"))
    parser.add_argument("--evidence-dir", type=Path, default=Path("target/qa-evidence"))
    args = parser.parse_args()
    args.evidence_dir.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="removent-tray-") as temporary:
        data = Path(temporary)
        settings = data / "settings.toml"
        settings.write_text('language = "En"\nhost_enabled = false\nhost_port = 0\nprevent_idle_sleep = false\n[authentication]\nmode = "pairing_code"\n')
        env = dict(os.environ, REMOVENT_DATA_DIR=str(data))
        with (args.evidence_dir / "tray.log").open("w") as log:
            tray = subprocess.Popen([str(args.bin_dir.resolve() / "removent-tray.exe")], env=env, stdout=log, stderr=log)
            try:
                hwnd = wait_for(lambda: window(tray.pid, "tray_icon_app"), "Tray owner window missing")
                def registered():
                    for identifier in range(32):
                        icon = IconIdentifier(c.sizeof(IconIdentifier), hwnd, identifier)
                        rect = w.RECT()
                        if shell.Shell_NotifyIconGetRect(c.byref(icon), c.byref(rect)) == 0:
                            return True
                    return False
                wait_for(registered, "No tray icon registered with Explorer")
                def click(label):
                    user.SetCursorPos(500, 400)
                    user.PostMessageW(hwnd, 6002, 0, 0x205)  # tray-icon shell callback, right button up
                    popup = wait_for(lambda: window(tray.pid, "#32768", visible=True), "Native tray menu missing")
                    menu = user.SendMessageW(popup, 0x1E1, 0, 0)  # MN_GETHMENU
                    def item():
                        for index in range(user.GetMenuItemCount(menu)):
                            text = c.create_unicode_buffer(256)
                            user.GetMenuStringW(menu, index, text, 256, 0x400)
                            if text.value.replace("&", "") == label and not user.GetMenuState(menu, index, 0x400) & 3:
                                return index + 1
                        return None
                    index = wait_for(item, f"Enabled menu item missing: {label}") - 1
                    rect = w.RECT()
                    assert user.GetMenuItemRect(None, menu, index, c.byref(rect))
                    user.SetCursorPos((rect.left + rect.right) // 2, (rect.top + rect.bottom) // 2)
                    user.mouse_event(0x2, 0, 0, 0, 0)
                    user.mouse_event(0x4, 0, 0, 0, 0)
                    time.sleep(0.3)
                click("Start background service")
                click("Share this desktop")
                wait_for(lambda: tomllib.loads(settings.read_text())["host_enabled"], "Sharing was not enabled")
                click("Show pairing code")
                dialog = wait_for(lambda: window(tray.pid, title="Pairing code", visible=True), "Native pairing dialog missing")
                ImageGrab.grab().save(args.evidence_dir / "tray-pairing.png")
                user.SetForegroundWindow(dialog)
                user.keybd_event(0x0D, 0, 0, 0)
                user.keybd_event(0x0D, 0, 2, 0)
                wait_for(lambda: not window(tray.pid, title="Pairing code", visible=True), "Pairing dialog did not close")
                click("Share this desktop")
                wait_for(lambda: not tomllib.loads(settings.read_text())["host_enabled"], "Sharing was not disabled")
                click("Stop background service")
                wait_for(lambda: (data / "run/service-stopped").exists(), "Stop intent missing")
                click("Quit tray")
                assert tray.wait(timeout=5) == 0
            finally:
                if tray.poll() is None:
                    subprocess.run(["taskkill", "/PID", str(tray.pid), "/T", "/F"], check=False, stdout=log, stderr=log)
                    tray.wait(timeout=5)
    print("PASS: Explorer icon registration, native menu clicks, sharing, pairing dialog, service stop and tray quit")


if __name__ == "__main__":
    main()
