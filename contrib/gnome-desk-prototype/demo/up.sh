#!/bin/bash
# Bring up the whole private demo (everything tracked by pid files): up.sh [settle seconds]
S=${DEMO_DIR:?set DEMO_DIR to a scratch directory}
mkdir -p $S/pids $S/voice-out; rm -f $S/voice-out/*
cp $S/tools/demo/site/pricing-before.html $S/tools/demo/site/pricing.html
if ! { [ -f $S/pids/http.pid ] && kill -0 $(cat $S/pids/http.pid) 2>/dev/null; }; then
  (cd $S/tools/demo/site && setsid nohup python3 -m http.server 8099 --bind 127.0.0.1 > $S/site.log 2>&1 & echo $! > $S/pids/http.pid)
fi
if ! { [ -f $S/pids/xvfb.pid ] && kill -0 $(cat $S/pids/xvfb.pid) 2>/dev/null; }; then $S/vnc-fixture.sh start; fi
cp $(dirname "$0")/../extension/horizon-desk@horizon/extension.js ${DEMO_DIR}/gnd/home/.local/share/gnome-shell/extensions/horizon-desk@horizon/extension.js
$S/gnome-restart.sh >/dev/null
export DBUS_SESSION_BUS_ADDRESS="$(cat ${DEMO_DIR}/gnd/bus.addr)" HOME=${DEMO_DIR}/gnd/home XDG_RUNTIME_DIR=${DEMO_DIR}/gnd/rt
for k in picture-uri picture-uri-dark; do gsettings set org.gnome.desktop.background $k "file://$S/wall.png"; done
gsettings set org.gnome.shell enabled-extensions "['horizon-desk@horizon', 'ubuntu-dock@ubuntu.com']"
D=org.gnome.shell.extensions.dash-to-dock
gsettings set $D dock-position BOTTOM; gsettings set $D extend-height false; gsettings set $D dock-fixed true
gsettings set $D dash-max-icon-size 52; gsettings set $D transparency-mode FIXED; gsettings set $D background-opacity 0.55
gsettings set $D show-trash false; gsettings set $D show-mounts false; gsettings set $D height-fraction 0.9
$S/horizon-restart.sh ${1:-50} >/dev/null
