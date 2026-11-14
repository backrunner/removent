"""X11 pointer input with AT-SPI used only to locate native controls."""
import os
import re
from pathlib import Path
import subprocess
import time

import dbus
import pyatspi
from gi.repository import GLib
from PIL import ImageGrab
from tray_lifecycle import wait_for


def xdo(*args, check=True):
    return subprocess.run(["xdotool", *map(str, args)], capture_output=True, text=True, check=check, timeout=10)


def descendants(root, depth=0):
    if depth > 12:
        return
    try:
        root.clearCache()
        yield root
        for child in root:
            yield from descendants(child, depth + 1)
    except (RuntimeError, LookupError):
        return


def showing(node):
    try:
        return node.getState().contains(pyatspi.STATE_SHOWING)
    except (RuntimeError, LookupError):
        return False


def press(node, scale):
    rect = node.queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
    assert rect.width > 0 and rect.height > 0, (node.name, rect)
    xdo("mousemove", (rect.x + rect.width // 2) * scale, (rect.y + rect.height // 2) * scale, "click", 1)


class LinuxDesktop:
    def __init__(self, root, env, args, log):
        self.env, self.args, self.log = env, args, log
        self.panel = None
        self.pid = None
        config = root / "config/xfce4/xfconf/xfce-perchannel-xml"
        config.mkdir(parents=True)
        (config / "xfce4-panel.xml").write_text('''<?xml version="1.0"?>
<channel name="xfce4-panel" version="1.0">
  <property name="configver" type="int" value="2"/>
  <property name="panels" type="array"><value type="int" value="1"/>
    <property name="panel-1" type="empty">
      <property name="position" type="string" value="p=6;x=0;y=0"/>
      <property name="length" type="uint" value="100"/>
      <property name="position-locked" type="bool" value="true"/>
      <property name="size" type="uint" value="32"/>
      <property name="plugin-ids" type="array"><value type="int" value="1"/></property>
    </property>
  </property>
  <property name="plugins" type="empty"><property name="plugin-1" type="string" value="systray"/></property>
</channel>''')
        env.update(XDG_CONFIG_HOME=str(root / "config"), GTK_MODULES="gail:atk-bridge",
                   NO_AT_BRIDGE="0", GDK_SCALE=str(args.scale),
                   GTK_THEME="Adwaita:dark" if args.theme == "dark" else "Adwaita")

    def __enter__(self):
        subprocess.run(["dbus-update-activation-environment", "XDG_CONFIG_HOME", "GTK_MODULES", "NO_AT_BRIDGE", "GDK_SCALE", "GTK_THEME"], env=self.env, check=True)
        self.bus = dbus.SessionBus()
        self.start_panel()
        return self

    def start_panel(self):
        self.panel = subprocess.Popen(["xfce4-panel", "--disable-wm-check"], env=self.env,
                                      stdout=self.log, stderr=self.log)
        wait_for(lambda: self.bus.name_has_owner("org.kde.StatusNotifierWatcher"), "Desktop panel did not start")

    def __exit__(self, error_type, *_):
        if error_type is not None:
            self.snapshot("failure")
            for node in self.nodes():
                self.log.write(f"AT-SPI {node.getRoleName()}: {node.name!r}, showing={showing(node)}\n")
            self.log.flush()
        for window in self.app_windows():
            self.close_app(window)
        if self.panel and self.panel.poll() is None:
            self.panel.terminate()
            self.panel.wait(timeout=5)

    def attach(self, pid):
        self.pid = pid

    def registered(self):
        try:
            watcher = self.bus.get_object("org.kde.StatusNotifierWatcher", "/StatusNotifierWatcher")
            entries = watcher.Get("org.kde.StatusNotifierWatcher", "RegisteredStatusNotifierItems", dbus_interface="org.freedesktop.DBus.Properties")
            return bool(entries)
        except dbus.DBusException:
            return False

    def nodes(self):
        context = GLib.MainContext.default()
        while context.pending():
            context.iteration(False)
        return descendants(pyatspi.Registry.getDesktop(0))

    def click(self, label):
        # The fixture panel contains only this tray item. Query its actual AT-SPI
        # bounds rather than invoking the exported D-Bus menu callbacks.
        def icon():
            candidates = []
            for node in self.nodes():
                if showing(node) and node.getRole() == pyatspi.ROLE_PUSH_BUTTON:
                    if "removent" in (node.name or "").lower():
                        return node
                    if node.getApplication().name == "xfce4-panel":
                        candidates.append(node)
            # Xfce's StatusNotifier button has an empty accessible name. This
            # isolated panel has only one plugin and one registered tray icon.
            if len(candidates) == 1 and self.registered():
                return candidates[0]
            return None
        try:
            node = wait_for(icon, "Accessible tray icon missing")
        except AssertionError:
            self.snapshot("missing-icon")
            for node in self.nodes():
                self.log.write(f"AT-SPI {node.getRoleName()}: {node.name!r}, showing={showing(node)}\n")
            self.log.flush()
            raise
        rect = node.queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
        # GTK's AT-SPI screen coordinates are logical pixels under GDK_SCALE.
        xdo("mousemove", (rect.x + rect.width // 2) * self.args.scale,
            (rect.y + rect.height // 2) * self.args.scale, "click", 3)
        # Xfce's exported dbusmenu popup is not included in its AT-SPI tree.
        # Read labels/state only, then navigate the visible GTK menu with actual
        # keyboard input. Never invoke dbusmenu.Event or a widget callback.
        def entries():
            watcher = self.bus.get_object("org.kde.StatusNotifierWatcher", "/StatusNotifierWatcher")
            registered = watcher.Get("org.kde.StatusNotifierWatcher", "RegisteredStatusNotifierItems",
                                     dbus_interface="org.freedesktop.DBus.Properties")
            assert len(registered) == 1
            service, path = str(registered[0]).split("/", 1)
            item = self.bus.get_object(service, "/" + path)
            menu_path = item.Get("org.kde.StatusNotifierItem", "Menu", dbus_interface="org.freedesktop.DBus.Properties")
            menu = dbus.Interface(self.bus.get_object(service, menu_path), "com.canonical.dbusmenu")
            labels = [str(child[1].get("label", "")).replace("_", "")
                      for child in menu.GetLayout(0, -1, [])[1][2]
                      if child[1].get("enabled", True) and child[1].get("visible", True)
                      and child[1].get("type") != "separator"]
            return labels if label in labels else None
        labels = wait_for(entries, f"Enabled native menu item missing: {label}")
        time.sleep(0.2)
        self.snapshot("menu")
        xdo("key", "--clearmodifiers", "Home", *(["Down"] * labels.index(label)), "Return")
        time.sleep(0.3)

    def dialog(self, title):
        # GtkMessageDialog exposes its message type (e.g. "Information") as
        # the AT-SPI name; match the actual X11 title before inspecting controls.
        window = xdo("search", "--onlyvisible", "--pid", self.pid,
                     "--name", "^" + re.escape(title) + "$", check=False)
        if window.returncode:
            return None
        for node in self.nodes():
            if node.getRole() in (pyatspi.ROLE_DIALOG, pyatspi.ROLE_ALERT) and showing(node):
                if node.get_process_id() == self.pid:
                    return node
        return None

    def answer(self, title, allow):
        dialog = wait_for(lambda: self.dialog(title), "Native dialog missing")
        expected = ["OK", "确定"] if allow is None else ["Yes", "是", "允许"] if allow else ["No", "否", "拒绝"]
        buttons = [node for node in descendants(dialog) if node.getRole() == pyatspi.ROLE_PUSH_BUTTON and showing(node)]
        button = next((node for node in buttons if (node.name or "").replace("_", "") in expected), None)
        assert button is not None, [node.name for node in buttons]
        press(button, self.args.scale)

    def accept_default(self, title):
        window = xdo("search", "--onlyvisible", "--pid", self.pid,
                     "--name", "^" + re.escape(title) + "$").stdout.strip().splitlines()[0]
        xdo("windowactivate", "--sync", window, "key", "Return")

    def app_windows(self):
        value = xdo("search", "--onlyvisible", "--name", "^Removent$", check=False)
        found = []
        for window in value.stdout.strip().splitlines() if value.returncode == 0 else []:
            result = xdo("getwindowgeometry", "--shell", window, check=False)
            if result.returncode == 0:
                geometry = dict(line.split("=", 1) for line in result.stdout.splitlines())
                if int(geometry["WIDTH"]) > 500 and int(geometry["HEIGHT"]) > 400:
                    found.append(window)
        return found

    def app_window(self):
        return next(iter(self.app_windows()), None)

    def app_count(self):
        return len(self.app_windows())

    def app_position(self, window):
        geometry = dict(line.split("=", 1) for line in xdo("getwindowgeometry", "--shell", window).stdout.splitlines())
        return int(geometry["X"]), int(geometry["Y"])

    def move_to_secondary(self, window):
        result = subprocess.run(["xrandr", "--listmonitors"], capture_output=True, text=True, check=True)
        (self.args.evidence_dir / "monitors.txt").write_text(result.stdout)
        monitors = re.findall(r"(\d+)/\d+x(\d+)/\d+\+(\d+)\+(\d+)", result.stdout)
        assert len(monitors) == 2, result.stdout
        x, y = map(int, monitors[1][2:])
        xdo("windowsize", window, 1000, 680, "windowmove", window, x + 40, y + 40)
        wait_for(lambda: self.app_position(window)[0] >= x, "Desktop did not move to the second monitor")
        time.sleep(0.5)
        return self.app_position(window)

    def minimize(self, window):
        xdo("windowminimize", "--sync", window)
        wait_for(lambda: window not in self.app_windows(), "Desktop did not minimize")

    def app_active(self, window):
        return xdo("getactivewindow", check=False).stdout.strip() == window and window in self.app_windows()

    def close_app(self, window):
        xdo("windowactivate", "--sync", window, "key", "alt+F4", check=False)

    def restart_shell(self):
        self.panel.terminate()
        self.panel.wait(timeout=5)
        wait_for(lambda: not self.bus.name_has_owner("org.kde.StatusNotifierWatcher"), "Old panel watcher remained")
        self.start_panel()
        time.sleep(0.5)

    def snapshot(self, name):
        ImageGrab.grab().save(self.args.evidence_dir / f"{name}.png")
