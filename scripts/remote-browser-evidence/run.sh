#!/usr/bin/env bash
# Headless driver for the phase 6 live evidence run: a private Xvfb display
# and window manager, then the MCP flow. Only the processes this script
# started are cleaned up, on any exit.
set -u
S="$(cd "$(dirname "$0")" && pwd)"
BIN=${1:?horizon binary}
shift

display=""
for n in $(seq 99 140); do
  if [ ! -e "/tmp/.X${n}-lock" ] && [ ! -e "/tmp/.X11-unix/X${n}" ]; then
    display=":${n}"
    break
  fi
done
[ -n "$display" ] || { echo "no free X display between :99 and :140" >&2; exit 1; }
export DISPLAY="$display"

XVFB=""; WM=""
cleanup() {
  [ -n "$WM" ] && kill "$WM" 2>/dev/null
  [ -n "$XVFB" ] && kill "$XVFB" 2>/dev/null
}
trap cleanup EXIT INT TERM HUP

Xvfb "$display" -screen 0 1600x1000x24 >/dev/null 2>&1 & XVFB=$!
for _ in $(seq 1 30); do [ -e "/tmp/.X11-unix/X${display#:}" ] && break; sleep 0.1; done
openbox >/dev/null 2>&1 & WM=$!
sleep 0.5
python3 "$S/live_smoke.py" --horizon "$BIN" --display "$display" "$@"
