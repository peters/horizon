#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
DATA_HOME="${XDG_DATA_HOME:-"$HOME/.local/share"}"
APPLICATIONS_DIR="$DATA_HOME/applications"
ICONS_DIR="$DATA_HOME/icons/hicolor"
DESKTOP_FILE="$APPLICATIONS_DIR/horizon.desktop"

install -d "$APPLICATIONS_DIR"
install -d "$ICONS_DIR/scalable/apps"

for size in 64 128 256 512; do
  install -d "$ICONS_DIR/${size}x${size}/apps"
  install -m 0644 "$ROOT_DIR/assets/icons/icon-${size}.png" "$ICONS_DIR/${size}x${size}/apps/horizon.png"
done

install -m 0644 "$ROOT_DIR/assets/icons/logo.svg" "$ICONS_DIR/scalable/apps/horizon.svg"
install -m 0644 "$ROOT_DIR/packaging/linux/horizon.desktop" "$DESKTOP_FILE"

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$APPLICATIONS_DIR"
fi

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -f -q "$ICONS_DIR"
fi

printf 'Installed Horizon desktop assets to %s\n' "$DATA_HOME"
