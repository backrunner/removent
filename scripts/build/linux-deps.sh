#!/usr/bin/env bash
set -euo pipefail
# Development and CI dependencies for the desktop, capture, input, and tray.
apt-get update
apt-get install -y --no-install-recommends build-essential cmake clang libclang-dev pkg-config nasm \
  libssl-dev libopus-dev libpam0g-dev libasound2-dev libudev-dev libfontconfig1-dev \
  libx11-dev libxi-dev libxrandr-dev libxinerama-dev libxcursor-dev libxkbcommon-dev \
  libxkbcommon-x11-dev libxcb1-dev libxcb-shape0-dev libxcb-xfixes0-dev libxfixes-dev \
  libxtst-dev libxdo-dev libwayland-dev libegl1-mesa-dev libgbm-dev libpipewire-0.3-dev \
  libgtk-3-dev libayatana-appindicator3-dev libdbus-1-dev \
  xvfb xauth dbus-x11 openbox xdotool python3-tk python3-pil python3-dbus python3-gi \
  python3-pyatspi python3-psutil at-spi2-core fonts-noto-cjk \
  xfce4-panel libvulkan1 mesa-vulkan-drivers gnome-keyring
