#!/usr/bin/env python3
"""Run the native windows tray lifecycle and remote-session acceptance."""
from tray_lifecycle import run
from tray_ui_windows import WindowsDesktop

if __name__ == "__main__":
    run(WindowsDesktop)
