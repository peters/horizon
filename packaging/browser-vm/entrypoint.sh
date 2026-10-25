#!/usr/bin/env bash
# Start the display, the window manager, the VNC server and the profile host of
# the browser image, and stop them together. README.md gives the mount points
# and the ports.

set -euo pipefail

readonly STATE_DIR="${HORIZON_BROWSER_VM_STATE:-/state}"
readonly DISPLAY_NUMBER="${HORIZON_BROWSER_VM_DISPLAY:-99}"
readonly VNC_PORT="${HORIZON_BROWSER_VM_VNC_PORT:-5900}"
readonly SCREEN="${HORIZON_BROWSER_VM_SCREEN:-1920x1080x24}"
readonly HOST_FILE="${STATE_DIR}/host.yaml"
readonly EXAMPLE_HOST_FILE="/usr/local/share/horizon-browser-vm/host.example.yaml"

export DISPLAY=":${DISPLAY_NUMBER}"
export HORIZON_BROWSER_ROOT="${STATE_DIR}/browser-root"

xvfb_pid=""
dbus_pid=""
wm_pid=""
vnc_pid=""
host_pid=""

log() {
    printf 'browser-vm: %s\n' "$*" >&2
}

# Stop one child. A process that ignores the termination signal is killed.
# shellcheck disable=SC2329  # called from the trap of the exit signal
terminate() {
    local pid="${1:-}"
    if [[ -z "$pid" ]]; then
        return 0
    fi
    kill -TERM "$pid" 2>/dev/null || return 0
    for _ in {1..25}; do
        if ! kill -0 "$pid" 2>/dev/null; then
            return 0
        fi
        sleep 0.2
    done
    log "process $pid ignores the termination signal, kill it"
    kill -KILL "$pid" 2>/dev/null || true
}

# Stop every child when the container stops.
# shellcheck disable=SC2329  # called through the trap below
cleanup() {
    trap - EXIT
    set +e
    if [[ -n "$host_pid" ]]; then
        log "stop the profile host"
        terminate "$host_pid"
        wait "$host_pid" 2>/dev/null
    fi
    terminate "$vnc_pid"
    terminate "$wm_pid"
    terminate "$dbus_pid"
    terminate "$xvfb_pid"
    log "the browser machine stopped"
}

trap 'exit 0' TERM
trap 'exit 0' INT
trap cleanup EXIT

mkdir -p "$STATE_DIR" "$HORIZON_BROWSER_ROOT"
if [[ ! -f "$HOST_FILE" ]]; then
    install -m 0600 "$EXAMPLE_HOST_FILE" "$HOST_FILE"
    log "wrote the example host file to $HOST_FILE"
fi

Xvfb "$DISPLAY" -screen 0 "$SCREEN" -nolisten tcp -ac >/dev/null 2>&1 &
xvfb_pid=$!
display_ready=0
for _ in {1..100}; do
    if [[ -S "/tmp/.X11-unix/X${DISPLAY_NUMBER}" ]]; then
        display_ready=1
        break
    fi
    if ! kill -0 "$xvfb_pid" 2>/dev/null; then
        break
    fi
    sleep 0.1
done
if [[ "$display_ready" -ne 1 ]]; then
    log "the display $DISPLAY did not start"
    exit 1
fi
log "the display $DISPLAY runs ${SCREEN}"

# A session bus keeps Chromium and the window manager quiet, and it gives the
# browser the services that it asks for.
if command -v dbus-launch >/dev/null 2>&1; then
    eval "$(dbus-launch --sh-syntax)"
    dbus_pid="${DBUS_SESSION_BUS_PID:-}"
    export DBUS_SESSION_BUS_ADDRESS
    log "the session bus runs as $dbus_pid"
fi

openbox >/dev/null 2>&1 &
wm_pid=$!

# The VNC server listens on the network of the container. Publish the port on
# the loopback of the Docker host only, and reach it through SSH. The server
# has no password, so a published port is open access to the desktop.
x11vnc -display "$DISPLAY" -rfbport "$VNC_PORT" -nopw -forever -shared -quiet >/dev/null 2>&1 &
vnc_pid=$!
log "the VNC server listens on the container port $VNC_PORT"

log "start the profile host with $HOST_FILE"
horizon-browser host --config "$HOST_FILE" --visible &
host_pid=$!
log "the profile host runs as $host_pid, the browser runtime root is $HORIZON_BROWSER_ROOT"

set +e
wait "$host_pid"
status=$?
set -e
host_pid=""

# A termination signal stops the container through the trap above. The host
# stops by itself only after an error.
if [[ "$status" -eq 0 ]]; then
    log "the profile host stopped without a signal, stop the container"
    exit 1
fi
log "the profile host stopped with the status $status"
exit "$status"
