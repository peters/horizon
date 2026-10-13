#!/bin/bash
# The Upkeep loop on a cloud worker, as the agent account: the session's environment, then
# the scheduler in run.py.
set -u
session="${1:?session required}"
export HOME=/workspace/home DISPLAY=:99 HORIZON_BROWSER_ROOT=/workspace/home/.horizon
export HORIZON_CLOUD_ACTOR="cloud:$session" HORIZON_BROWSER_ACTOR="horizon:cloud-$session"
export HORIZON_DEVICE_ACTOR="horizon:cloud-$session"
export HORIZON_SESSION_DIR="/workspace/session-data/$session"
unset HORIZON HORIZON_BROWSER_HOST_INSTANCE
if host_value="$(cat /run/horizon-worker/browser-host-instance 2>/dev/null)" && [ -n "$host_value" ]; then
    export HORIZON_BROWSER_HOST_INSTANCE="$host_value"
fi
exec python3 -I -u /workspace/home/upkeep/run.py
