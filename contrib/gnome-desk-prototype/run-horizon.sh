#!/bin/bash
# Run Horizon in the private GNOME session in desk mode.
S=${DEMO_DIR:?set DEMO_DIR to a scratch directory}
export HOME=${DEMO_DIR:?set DEMO_DIR}/gnd/home XDG_RUNTIME_DIR=${DEMO_DIR:?set DEMO_DIR}/gnd/rt
export WAYLAND_DISPLAY=wayland-demo DBUS_SESSION_BUS_ADDRESS="$(cat ${DEMO_DIR:?set DEMO_DIR}/gnd/bus.addr)"
unset DISPLAY
export HORIZON_DESK_SCRIPT=$S/tools/demo/cmd.txt HORIZON_DEMO_ASSISTANT=$S/tools/demo/assistant.sh HORIZON_STUB_STEP=1.4
export PATH=$S/tools/bin:/usr/local/bin:/usr/bin:/bin HORIZON_MCP_BIN=$S/tools/horizon-m5 HORIZON_DESK_MODE=1
cd /tmp
exec $S/tools/horizon-m5 --config $S/tools/demo/horizon.yaml --ephemeral
