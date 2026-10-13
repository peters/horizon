#!/bin/bash
# The Upkeep loop on a cloud worker, as the agent account: the session's environment, then
# the scheduler in run.py.
set -u
session="${1:?session required}"
export HOME=/workspace/home DISPLAY=:99 HORIZON_BROWSER_ROOT=/workspace/home/.horizon
export HORIZON_CLOUD_ACTOR="cloud:$session" HORIZON_BROWSER_ACTOR="horizon:cloud-$session"
export HORIZON_DEVICE_ACTOR="horizon:cloud-$session"
export HORIZON_SESSION_DIR="/workspace/session-data/$session"
# Shared by every session on this worker; see horizon-worker-gpu-lock.
mkdir -p /workspace/locks
export HORIZON_GPU_LOCK=/workspace/locks/gpu.lock
# run.py gives each agent the browser host instance that the worker publishes at that time.
unset HORIZON HORIZON_BROWSER_HOST_INSTANCE
# The loop runs a copy, so it knows exactly which files run and sees a newer install.
active=/workspace/home/upkeep/.active
mkdir -p "$active"
cp /workspace/home/upkeep/run.sh /workspace/home/upkeep/run.py "$active/"
exec python3 -I -u "$active/run.py" "$session"
