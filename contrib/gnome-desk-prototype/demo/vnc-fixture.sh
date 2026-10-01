#!/bin/bash
# A small isolated X desktop with a native app, served over VNC for a Horizon Device panel. Tracked by pid files.
S=${DEMO_DIR:?set DEMO_DIR to a scratch directory}
export HOME=$S/vnchome XDG_RUNTIME_DIR=$S/vnchome/rt GSK_RENDERER=cairo GDK_BACKEND=x11 DISPLAY=:97
unset WAYLAND_DISPLAY DBUS_SESSION_BUS_ADDRESS
case "$1" in
 start)
  mkdir -p $HOME/rt && chmod 700 $HOME/rt
  (setsid nohup Xvfb :97 -screen 0 800x520x24 -nolisten tcp > $S/xvfb.log 2>&1 & echo $! > $S/pids/xvfb.pid); sleep 1.5
  xsetroot -solid '#0f1830' 2>/dev/null
  (setsid nohup openbox > $S/openbox.log 2>&1 & echo $! > $S/pids/openbox.pid); sleep 1
  (setsid nohup dbus-run-session -- gnome-calculator > $S/calc.log 2>&1 & echo $! > $S/pids/calc.pid); sleep 2
  (setsid nohup x11vnc -display :97 -localhost -desktop Simulator -rfbport 5997 -nopw -forever -shared -noxdamage -noshm > $S/x11vnc.log 2>&1 & echo $! > $S/pids/x11vnc.pid); sleep 1 ;;
 stop)
  for f in x11vnc calc openbox xvfb; do [ -f $S/pids/$f.pid ] && { kill -TERM $(cat $S/pids/$f.pid) 2>/dev/null; rm -f $S/pids/$f.pid; }; done ;;
esac
