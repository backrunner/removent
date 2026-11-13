#!/usr/bin/env python3
"""Exercise real AppIndicator menu callbacks and a GTK dialog in an isolated X11 session."""
import argparse
import os
from pathlib import Path
import subprocess
import tempfile
import time
import tomllib

import dbus
from PIL import ImageGrab


def wait_for(check, message, seconds=12):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            value = check()
            if value:
                return value
        except dbus.DBusException:
            pass
        time.sleep(0.1)
    raise AssertionError(message)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--bin-dir", type=Path, default=Path("target/release"))
    parser.add_argument("--evidence-dir", type=Path, default=Path("target/qa-evidence"))
    args = parser.parse_args()
    args.evidence_dir.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="removent-tray-") as temporary:
        root = Path(temporary)
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
        data = root / "data"
        data.mkdir()
        settings = data / "settings.toml"
        settings.write_text('language = "En"\nhost_enabled = false\nhost_port = 0\nprevent_idle_sleep = false\n[authentication]\nmode = "pairing_code"\n')
        env = dict(os.environ, XDG_CONFIG_HOME=str(root / "config"), REMOVENT_DATA_DIR=str(data))
        subprocess.run(["dbus-update-activation-environment", "XDG_CONFIG_HOME"], env=env, check=True)
        with (args.evidence_dir / "tray.log").open("w") as log:
            panel = subprocess.Popen(["xfce4-panel", "--disable-wm-check"], env=env, stdout=log, stderr=log)
            bus = dbus.SessionBus()
            # Start against a ready desktop, avoiding AppIndicator's temporary
            # XEmbed fallback while the test panel is still being initialized.
            wait_for(lambda: bus.name_has_owner("org.kde.StatusNotifierWatcher"), "Desktop panel did not start")
            tray = subprocess.Popen([str(args.bin_dir.resolve() / "removent-tray")], env=env, stdout=log, stderr=log)
            try:
                def items():
                    watcher = bus.get_object("org.kde.StatusNotifierWatcher", "/StatusNotifierWatcher")
                    return watcher.Get("org.kde.StatusNotifierWatcher", "RegisteredStatusNotifierItems", dbus_interface="org.freedesktop.DBus.Properties")
                identifier = str(wait_for(items, "Tray was not registered with the desktop panel")[0])
                service, path = identifier.split("/", 1)
                item = bus.get_object(service, "/" + path)
                menu_path = item.Get("org.kde.StatusNotifierItem", "Menu", dbus_interface="org.freedesktop.DBus.Properties")
                menu = dbus.Interface(bus.get_object(service, menu_path), "com.canonical.dbusmenu")
                def entries():
                    layout = menu.GetLayout(0, -1, [])
                    return {str(child[1].get("label", "")).replace("_", ""): (child[0], child[1]) for child in layout[1][2]}
                def click(label):
                    identifier, properties = entries()[label]
                    assert properties.get("enabled", True), f"Menu item disabled: {label}"
                    menu.Event(identifier, "clicked", dbus.Int32(0), dbus.UInt32(0))
                click("Start background service")
                wait_for(lambda: entries()["Share this desktop"][1].get("enabled", True), "Daemon did not connect to tray")
                click("Share this desktop")
                wait_for(lambda: tomllib.loads(settings.read_text())["host_enabled"], "Tray did not enable sharing")
                click("Show pairing code")
                def dialog():
                    result = subprocess.run(["xdotool", "search", "--onlyvisible", "--name", "^Pairing code$"], capture_output=True, text=True)
                    return result.stdout.strip().splitlines()[0] if result.returncode == 0 else None
                window = wait_for(dialog, "Native pairing dialog did not appear")
                time.sleep(2)  # Let software rendering present the complete dialog.
                ImageGrab.grab().save(args.evidence_dir / "tray-pairing.png")
                subprocess.run(["xdotool", "windowactivate", "--sync", window, "key", "Return"], check=True)
                wait_for(lambda: not dialog(), "Pairing dialog did not close")
                click("Share this desktop")
                wait_for(lambda: not tomllib.loads(settings.read_text())["host_enabled"], "Tray did not disable sharing")
                click("Stop background service")
                wait_for(lambda: entries()["Start background service"][1].get("enabled", True), "Tray did not observe daemon shutdown")
                assert (data / "run/service-stopped").exists(), "Stop intent missing"
                click("Quit tray")
                assert tray.wait(timeout=5) == 0
            finally:
                # All descendants belong to this isolated data directory.
                subprocess.run(["pkill", "-TERM", "-P", str(tray.pid)], check=False)
                for process in [tray, panel]:
                    if process.poll() is None:
                        process.terminate()
                        process.wait(timeout=5)
    print("PASS: panel registration, tray start, sharing toggles, native pairing dialog, stop and quit")


if __name__ == "__main__":
    main()
