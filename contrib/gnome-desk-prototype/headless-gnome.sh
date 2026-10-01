#!/bin/bash
# Private headless GNOME Shell for the workspaces prototype. Nothing here touches the live session.
export XDG_RUNTIME_DIR=${DEMO_DIR:?set DEMO_DIR}/gnd/rt HOME=${DEMO_DIR:?set DEMO_DIR}/gnd/home
export XDG_CONFIG_HOME=$HOME/.config XDG_DATA_HOME=$HOME/.local/share XDG_CACHE_HOME=$HOME/.cache
unset WAYLAND_DISPLAY DISPLAY DBUS_SESSION_BUS_ADDRESS XDG_SESSION_ID
exec dbus-run-session -- gnome-shell --headless --wayland --no-x11 --wayland-display=wayland-demo --virtual-monitor 1600x1000 --unsafe-mode
