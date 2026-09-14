#!/usr/bin/env bash
# Headless driver for the phase 6 live evidence run: Xvfb + openbox, then the MCP flow.
set -u
S="$(cd "$(dirname "$0")" && pwd)"
BIN=${1:?horizon binary}
shift
export DISPLAY=:99
pkill -f "[X]vfb :99" 2>/dev/null; sleep 0.5
Xvfb :99 -screen 0 1600x1000x24 >/dev/null 2>&1 & XVFB=$!
sleep 1.5; openbox >/dev/null 2>&1 & WM=$!; sleep 0.5
python3 "$S/live_smoke.py" --horizon "$BIN" "$@"
STATUS=$?
kill "$WM" 2>/dev/null; kill "$XVFB" 2>/dev/null
exit $STATUS
