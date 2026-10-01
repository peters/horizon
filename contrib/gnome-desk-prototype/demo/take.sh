#!/bin/bash
# One full take: fresh shell and Horizon, record, play the story, stop. Helpers are tracked by pid file.
S=${DEMO_DIR:?set DEMO_DIR to a scratch directory}
cp $S/tools/demo/site/pricing-before.html $S/tools/demo/site/pricing.html
if ! { [ -f $S/pids/http.pid ] && kill -0 $(cat $S/pids/http.pid) 2>/dev/null; }; then
  (cd $S/tools/demo/site && setsid nohup python3 -m http.server 8099 --bind 127.0.0.1 > $S/site.log 2>&1 & echo $! > $S/pids/http.pid)
fi
if ! { [ -f $S/pids/xvfb.pid ] && kill -0 $(cat $S/pids/xvfb.pid) 2>/dev/null; }; then $S/vnc-fixture.sh start; fi
$S/gnome-restart.sh >/dev/null
export DBUS_SESSION_BUS_ADDRESS="$(cat ${DEMO_DIR}/gnd/bus.addr)" HOME=${DEMO_DIR}/gnd/home XDG_RUNTIME_DIR=${DEMO_DIR}/gnd/rt
for k in picture-uri picture-uri-dark; do gsettings set org.gnome.desktop.background $k "file://$S/wall.png"; done
gsettings set org.gnome.shell enabled-extensions "['horizon-desk@horizon']"

$S/horizon-restart.sh 50 >/dev/null
(setsid nohup python3 $S/record.py > $S/record.log 2>&1 &)
sleep 1
bash $S/story3.sh
touch $S/frames/STOP
sleep 3
cat $S/record.log
