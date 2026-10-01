#!/bin/bash
# (Re)start the private headless GNOME Shell. Only the process this script started is ever stopped.
S=${DEMO_DIR:?set DEMO_DIR to a scratch directory}
stop() { [ -f "$1" ] || return; P=$(cat "$1"); for c in $(pgrep -P "$P" 2>/dev/null); do kill -TERM "$c" 2>/dev/null; done; kill -TERM "$P" 2>/dev/null; rm -f "$1"; }
stop $S/pids/gnome.pid; sleep 3
rm -f ${DEMO_DIR}/gnd/rt/wayland-demo ${DEMO_DIR}/gnd/rt/wayland-demo.lock
(setsid nohup $S/gnome-session.sh > $S/gnome.log 2>&1 & echo $! > $S/pids/gnome.pid)
for i in $(seq 1 30); do [ -S ${DEMO_DIR}/gnd/rt/wayland-demo ] && break; sleep 1; done
sleep 5
W=$(cat $S/pids/gnome.pid); C=$(for c in $(pgrep -P $W); do tr "\0" " " < /proc/$c/cmdline | grep -q "^gnome-shell" && echo $c; done | head -1)
tr '\0' '\n' < /proc/$C/environ | grep ^DBUS_SESSION_BUS_ADDRESS | cut -d= -f2- > ${DEMO_DIR}/gnd/bus.addr
cat ${DEMO_DIR}/gnd/bus.addr
