#!/bin/sh
# Ubuntu 22.04 build/runtime/QA dependencies; run explicitly on a Linux build host.
set -eu
if [ "$(id -u)" -eq 0 ]; then SUDO=""; else SUDO="sudo"; fi
$SUDO apt-get update
$SUDO apt-get install -y build-essential pkg-config python3 curl openssl ca-certificates \
  libxkbcommon-dev libxkbcommon-x11-0 libwayland-dev libudev-dev libfontconfig1-dev libssl-dev \
  libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libgl1-mesa-dev \
  xdg-utils desktop-file-utils cups-client fonts-liberation fonts-dejavu-core \
  fonts-noto-cjk mesa-vulkan-drivers xvfb dbus-x11 x11-utils xdg-desktop-portal xdg-desktop-portal-gtk
