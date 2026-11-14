#!/usr/bin/env python3
"""Run the native linux tray lifecycle and remote-session acceptance."""
from tray_lifecycle import run
from tray_ui_linux import LinuxDesktop

if __name__ == "__main__":
    run(LinuxDesktop)
