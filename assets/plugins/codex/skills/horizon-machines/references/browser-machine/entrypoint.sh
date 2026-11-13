#!/bin/sh
# Start the display, the window manager, the loopback bridge and the headed
# Chromium of a browser machine, and keep them together until the container
# stops.
#
# Chromium listens on the loopback of this container, on BM_CDP_PORT. The bridge
# listens on the container interface, on BM_BRIDGE_PORT, and carries the
# published port of the Docker host to that loopback port. The published port
# therefore never needs Chromium to bind an interface.
#
# Chromium runs as root with --no-sandbox: Docker's default seccomp profile
# refuses the user namespaces that the Chromium sandbox needs.
#
# The entrypoint exits non-zero, with one line, when Chromium does not answer on
# the bridge port within 20 seconds. `docker ps` then shows the container as
# exited, and the wrapper on the host reports that line.
#
# With BM_ENGINE=firefox the container starts no display and no Chromium: Firefox
# runs headless, started by the driver of Horizon that this computer runs inside
# the container (`docker exec -i -e HOME=/profile <container>
# /opt/horizon/horizon --browser-host-driver`). The container then only keeps the
# profile mount and stays up.

set -eu

BM_PROFILE_DIR=${BM_PROFILE_DIR:-/profile}
BM_ENGINE=${BM_ENGINE:-chromium}
BM_DISPLAY=${BM_DISPLAY:-99}
BM_SCREEN=${BM_SCREEN:-1920x1080x24}
BM_WINDOW_SIZE=${BM_WINDOW_SIZE:-1600,900}
BM_CDP_PORT=${BM_CDP_PORT:-9223}
BM_BRIDGE_PORT=${BM_BRIDGE_PORT:-9222}

# 40 steps of 0.5 s are the 20 seconds that Horizon allows for the start.
READY_STEPS=40
READY_INTERVAL=0.5
# 100 steps of 0.1 s wait for the display socket.
DISPLAY_STEPS=100

DISPLAY_SOCKET="/tmp/.X11-unix/X${BM_DISPLAY}"
DISPLAY_LOCK="/tmp/.X${BM_DISPLAY}-lock"
DISPLAY_LOG="/tmp/xvfb-${BM_DISPLAY}.log"
ENDPOINT_URL="http://127.0.0.1:${BM_BRIDGE_PORT}/json/version"

export DISPLAY=":${BM_DISPLAY}"

xvfb_pid=
wm_pid=
bridge_pid=
chromium_pid=

log() {
    printf 'browser-machine: %s\n' "$*" >&2
}

# Stop every child that this script started. A child that ignores TERM is left
# to tini, which ends the container when this script exits.
terminate_all() {
    if [ -n "$chromium_pid" ]; then
        kill -TERM "$chromium_pid" 2>/dev/null || true
    fi
    if [ -n "$bridge_pid" ]; then
        kill -TERM "$bridge_pid" 2>/dev/null || true
    fi
    if [ -n "$wm_pid" ]; then
        kill -TERM "$wm_pid" 2>/dev/null || true
    fi
    if [ -n "$xvfb_pid" ]; then
        kill -TERM "$xvfb_pid" 2>/dev/null || true
    fi
}

stop_container() {
    trap - TERM INT
    log "stopping the browser machine"
    terminate_all
    wait 2>/dev/null || true
    exit 0
}
trap stop_container TERM INT

fail() {
    log "$*"
    terminate_all
    exit 1
}

# The container owns the display files. A container that stopped without this
# path, for example one that Docker killed, keeps a lock file that names no
# process, and Xvfb then refuses to start.
if [ -e "$DISPLAY_LOCK" ] || [ -e "$DISPLAY_SOCKET" ]; then
    lock_owner=
    if [ -f "$DISPLAY_LOCK" ]; then
        lock_owner=$(tr -cd '0-9' <"$DISPLAY_LOCK" 2>/dev/null || true)
    fi
    if [ -z "$lock_owner" ] || ! kill -0 "$lock_owner" 2>/dev/null; then
        rm -f "$DISPLAY_LOCK" "$DISPLAY_SOCKET"
        log "removed the display files of an earlier run of $DISPLAY"
    fi
fi

# The firefox engine needs no display: the driver of Horizon starts a headless
# Firefox, and the profile folder is the mounted one.
if [ "$BM_ENGINE" = firefox ]; then
    log "the firefox engine runs headless; the driver of Horizon starts it inside this container"
    log "profile ${BM_PROFILE_DIR}"
    while :; do
        sleep 3600
    done
fi

Xvfb "$DISPLAY" -screen 0 "$BM_SCREEN" -nolisten tcp -ac >"$DISPLAY_LOG" 2>&1 &
xvfb_pid=$!

display_ready=0
steps=0
while [ "$steps" -lt "$DISPLAY_STEPS" ]; do
    if [ -S "$DISPLAY_SOCKET" ] && kill -0 "$xvfb_pid" 2>/dev/null; then
        display_ready=1
        break
    fi
    if ! kill -0 "$xvfb_pid" 2>/dev/null; then
        break
    fi
    steps=$((steps + 1))
    sleep 0.1
done
if [ "$display_ready" -ne 1 ]; then
    if [ -f "$DISPLAY_LOG" ]; then
        sed 's/^/browser-machine: Xvfb: /' "$DISPLAY_LOG" >&2 || true
    fi
    fail "the display $DISPLAY did not start"
fi
log "the display $DISPLAY runs $BM_SCREEN"

if command -v openbox >/dev/null 2>&1; then
    openbox >/dev/null 2>&1 &
    wm_pid=$!
fi

# The bridge is the only listener on the container interface. Chromium keeps its
# own port on the loopback, which the published port of the Docker host cannot
# reach directly.
socat "TCP-LISTEN:${BM_BRIDGE_PORT},fork,reuseaddr,bind=0.0.0.0" "TCP:127.0.0.1:${BM_CDP_PORT}" >/dev/null 2>&1 &
bridge_pid=$!

mkdir -p "$BM_PROFILE_DIR"
chmod 0700 "$BM_PROFILE_DIR" 2>/dev/null || true

# Headed, because Horizon starts a headed browser and keeps the window size that
# it is given. about:blank keeps one page for the first connect. The profile
# folder is the mount of the profile folder of the destination.
chromium --remote-debugging-port="$BM_CDP_PORT" \
    --remote-debugging-address=127.0.0.1 \
    --user-data-dir="$BM_PROFILE_DIR" \
    --no-sandbox \
    --disable-dev-shm-usage \
    --window-size="$BM_WINDOW_SIZE" \
    --no-first-run \
    --no-default-browser-check \
    --disable-features=TranslateUI \
    about:blank &
chromium_pid=$!

endpoint_ready=0
steps=0
while [ "$steps" -lt "$READY_STEPS" ]; do
    if curl --fail --silent --show-error --max-time 2 --output /dev/null "$ENDPOINT_URL" 2>/dev/null; then
        endpoint_ready=1
        break
    fi
    if ! kill -0 "$chromium_pid" 2>/dev/null; then
        fail "Chromium stopped before its DevTools endpoint answered"
    fi
    steps=$((steps + 1))
    sleep "$READY_INTERVAL"
done
if [ "$endpoint_ready" -ne 1 ]; then
    fail "Chromium did not answer on $ENDPOINT_URL within 20 seconds"
fi

log "Chromium answers on ${BM_BRIDGE_PORT} for the DevTools port ${BM_CDP_PORT}, profile ${BM_PROFILE_DIR}"

# Stay with Chromium: its exit ends the container, so a dead browser is visible
# as an exited container and not as a port that answers nothing.
if wait "$chromium_pid"; then
    terminate_all
    log "Chromium stopped"
    exit 0
fi
terminate_all
log "Chromium stopped with a failure"
exit 1
