#!/bin/bash
# Stop the Horizon this script started, install the latest build, start it again.
S=${DEMO_DIR:?set DEMO_DIR to a scratch directory}
if [ -f $S/pids/horizon.pid ]; then P=$(cat $S/pids/horizon.pid); kill -TERM $P 2>/dev/null; for i in $(seq 1 80); do kill -0 $P 2>/dev/null || break; sleep 0.5; done; rm -f $S/pids/horizon.pid; fi
sleep 1
cp ${HORIZON_BINARY:?set HORIZON_BINARY} $S/tools/horizon-m5 || exit 1
: > $S/tools/demo/cmd.txt
(setsid nohup $S/desk-run.sh > $S/desk.log 2>&1 & echo $! > $S/pids/horizon.pid)
sleep ${1:-25}
