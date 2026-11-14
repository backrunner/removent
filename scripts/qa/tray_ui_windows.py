"""Physical pointer operations on Explorer icons, native menus and dialog buttons."""
import ctypes as c
from ctypes import wintypes as w
import os
import subprocess
import time
import winreg

from PIL import ImageGrab
from pywinauto import Desktop
from tray_lifecycle import wait_for

user = c.WinDLL("user32", use_last_error=True)
shell = c.WinDLL("shell32", use_last_error=True)
callback = c.WINFUNCTYPE(w.BOOL, w.HWND, w.LPARAM)
for name, args in {
    "EnumWindows": [callback, w.LPARAM],
    "GetWindowThreadProcessId": [w.HWND, c.POINTER(w.DWORD)],
    "GetClassNameW": [w.HWND, w.LPWSTR, c.c_int],
    "GetWindowTextW": [w.HWND, w.LPWSTR, c.c_int],
    "GetWindowRect": [w.HWND, c.POINTER(w.RECT)],
    "GetClientRect": [w.HWND, c.POINTER(w.RECT)],
    "IsWindowVisible": [w.HWND], "IsIconic": [w.HWND],
    "ShowWindow": [w.HWND, c.c_int], "SetForegroundWindow": [w.HWND],
    "SendMessageW": [w.HWND, w.UINT, w.WPARAM, w.LPARAM],
    "GetMenuItemCount": [w.HMENU],
    "GetMenuStringW": [w.HMENU, w.UINT, w.LPWSTR, c.c_int, w.UINT],
    "GetMenuState": [w.HMENU, w.UINT, w.UINT],
    "GetMenuItemRect": [w.HWND, w.HMENU, w.UINT, c.POINTER(w.RECT)],
    "GetDlgItem": [w.HWND, c.c_int],
}.items():
    getattr(user, name).argtypes = args
user.SendMessageW.restype = w.LPARAM
user.GetDlgItem.restype = w.HWND
user.GetForegroundWindow.restype = w.HWND


class IconIdentifier(c.Structure):
    _fields_ = [("cbSize", w.DWORD), ("hWnd", w.HWND), ("uID", w.UINT), ("guidItem", c.c_ubyte * 16)]


shell.Shell_NotifyIconGetRect.argtypes = [c.POINTER(IconIdentifier), c.POINTER(w.RECT)]
shell.Shell_NotifyIconGetRect.restype = w.LONG


def windows(pid=None, class_name=None, title=None, visible=False, large=False):
    found = []
    @callback
    def visit(hwnd, _):
        owner, rect = w.DWORD(), w.RECT()
        user.GetWindowThreadProcessId(hwnd, c.byref(owner))
        name, text = c.create_unicode_buffer(256), c.create_unicode_buffer(256)
        user.GetClassNameW(hwnd, name, 256)
        user.GetWindowTextW(hwnd, text, 256)
        user.GetClientRect(hwnd, c.byref(rect))
        if ((pid is None or owner.value == pid) and (class_name is None or name.value == class_name)
                and (title is None or text.value == title) and (not visible or user.IsWindowVisible(hwnd))
                and (not large or rect.right > 500)):
            found.append(hwnd)
        return True
    user.EnumWindows(visit, 0)
    return found


def pointer(rect, button="left"):
    user.SetCursorPos((rect.left + rect.right) // 2, (rect.top + rect.bottom) // 2)
    time.sleep(0.15)
    down, up = (0x2, 0x4) if button == "left" else (0x8, 0x10)
    user.mouse_event(down, 0, 0, 0, 0)
    user.mouse_event(up, 0, 0, 0, 0)


class WindowsDesktop:
    def __init__(self, root, env, args, log):
        assert os.environ.get("GITHUB_ACTIONS") == "true", "Explorer tests require an ephemeral CI desktop"
        c.windll.shcore.SetProcessDpiAwareness(2)
        self.args, self.log, self.pid = args, log, None
        self.theme_key = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"
        self.original = {}

    def __enter__(self):
        with winreg.CreateKey(winreg.HKEY_CURRENT_USER, self.theme_key) as key:
            for name in ["AppsUseLightTheme", "SystemUsesLightTheme"]:
                try:
                    self.original[name] = winreg.QueryValueEx(key, name)[0]
                except FileNotFoundError:
                    self.original[name] = None
                winreg.SetValueEx(key, name, 0, winreg.REG_DWORD, int(self.args.theme == "light"))
        return self

    def __exit__(self, error_type, *_):
        if error_type is not None:
            self.snapshot("failure")
        for hwnd in self.app_windows():
            self.close_app(hwnd)
        with winreg.CreateKey(winreg.HKEY_CURRENT_USER, self.theme_key) as key:
            for name, value in self.original.items():
                if value is None:
                    try: winreg.DeleteValue(key, name)
                    except FileNotFoundError: pass
                else:
                    winreg.SetValueEx(key, name, 0, winreg.REG_DWORD, value)

    def attach(self, pid):
        self.pid = pid
        self.owner = wait_for(lambda: next(iter(windows(pid, "tray_icon_app")), None), "Tray owner window missing")

    def icon_rect(self):
        for identifier in range(32):
            rect = w.RECT()
            icon = IconIdentifier(c.sizeof(IconIdentifier), self.owner, identifier)
            if shell.Shell_NotifyIconGetRect(c.byref(icon), c.byref(rect)) == 0:
                return rect
        return None

    def registered(self):
        return self.icon_rect() is not None

    def popup(self):
        return next(iter(windows(self.pid, "#32768", visible=True)), None)

    def click(self, label):
        # Start from the actual Explorer icon, including its overflow UI. No
        # synthetic tray callback or menu command is used.
        rect = wait_for(self.icon_rect, "Explorer icon missing")
        pointer(rect, "right")
        time.sleep(0.25)
        if not self.popup():
            taskbar = Desktop(backend="uia").window(class_name="Shell_TrayWnd")
            buttons = taskbar.descendants(control_type="Button")
            overflow = next((b for b in buttons if "hidden icons" in b.window_text().lower()), None)
            assert overflow is not None, [(b.window_text(), str(b.rectangle())) for b in buttons]
            overflow.click_input()
            time.sleep(0.25)
            pointer(wait_for(self.icon_rect, "Overflow icon missing"), "right")
        popup = wait_for(self.popup, "Physical tray icon click did not open menu")
        menu = user.SendMessageW(popup, 0x1E1, 0, 0)
        def item():
            for index in range(user.GetMenuItemCount(menu)):
                text = c.create_unicode_buffer(256)
                user.GetMenuStringW(menu, index, text, 256, 0x400)
                if text.value.replace("&", "") == label and not user.GetMenuState(menu, index, 0x400) & 3:
                    rect = w.RECT()
                    assert user.GetMenuItemRect(None, menu, index, c.byref(rect))
                    return rect
            return None
        rect = wait_for(item, f"Enabled menu item missing: {label}")
        self.snapshot("menu")
        pointer(rect)
        wait_for(lambda: not self.popup(), "Menu remained open after pointer click")

    def dialog(self, title):
        return next(iter(windows(self.pid, "#32770", title, True)), None)

    def answer(self, title, allow):
        hwnd = wait_for(lambda: self.dialog(title), "Native dialog missing")
        button = user.GetDlgItem(hwnd, 1 if allow is None else 6 if allow else 7)
        assert button
        rect = w.RECT()
        assert user.GetWindowRect(button, c.byref(rect))
        pointer(rect)

    def accept_default(self, title):
        hwnd = wait_for(lambda: self.dialog(title), "Native dialog missing")
        user.SetForegroundWindow(hwnd)
        user.keybd_event(0x0D, 0, 0, 0)
        user.keybd_event(0x0D, 0, 2, 0)

    def app_windows(self):
        return windows(title="Removent", visible=True, large=True)

    def app_window(self):
        return next(iter(self.app_windows()), None)

    def app_count(self):
        return len(self.app_windows())

    def minimize(self, hwnd):
        user.ShowWindow(hwnd, 6)
        wait_for(lambda: user.IsIconic(hwnd), "Desktop did not minimize")

    def app_active(self, hwnd):
        return not user.IsIconic(hwnd) and user.GetForegroundWindow() == hwnd

    def close_app(self, hwnd):
        user.ShowWindow(hwnd, 9)
        user.SetForegroundWindow(hwnd)
        user.keybd_event(0x12, 0, 0, 0)
        user.keybd_event(0x73, 0, 0, 0)
        user.keybd_event(0x73, 0, 2, 0)
        user.keybd_event(0x12, 0, 2, 0)

    def restart_shell(self):
        subprocess.run(["taskkill", "/IM", "explorer.exe", "/F"], check=True, stdout=self.log, stderr=self.log)
        subprocess.Popen([os.path.join(os.environ["WINDIR"], "explorer.exe")], stdout=self.log, stderr=self.log)
        wait_for(lambda: bool(windows(class_name="Shell_TrayWnd")), "Explorer did not restart", 25)
        time.sleep(1)

    def snapshot(self, name):
        ImageGrab.grab().save(self.args.evidence_dir / f"{name}.png")
