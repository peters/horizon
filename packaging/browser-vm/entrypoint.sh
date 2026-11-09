#!/usr/bin/env bash
# Start the display, the window manager, the VNC server and the profile host of
# the browser image, and stop them together. README.md gives the mount points
# and the ports.

set -euo pipefail

readonly STATE_DIR="${HORIZON_BROWSER_VM_STATE:-/state}"
readonly DISPLAY_NUMBER="${HORIZON_BROWSER_VM_DISPLAY:-99}"
readonly VNC_PORT="${HORIZON_BROWSER_VM_VNC_PORT:-5900}"
readonly SCREEN="${HORIZON_BROWSER_VM_SCREEN:-1920x1080x24}"
readonly SSH_ENABLED="${HORIZON_BROWSER_VM_SSH:-0}"
readonly SSH_PORT="${HORIZON_BROWSER_VM_SSH_PORT:-2222}"
readonly SSH_PUBKEY="${HORIZON_BROWSER_VM_SSH_PUBKEY:-}"
readonly HOST_FILE="${STATE_DIR}/host.yaml"
readonly EXAMPLE_HOST_FILE="/usr/local/share/horizon-browser-vm/host.example.yaml"

export DISPLAY=":${DISPLAY_NUMBER}"
export HORIZON_BROWSER_ROOT="${STATE_DIR}/browser-root"

xvfb_pid=""
dbus_pid=""
wm_pid=""
vnc_pid=""
sshd_pid=""
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
    terminate "$sshd_pid"
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

# The container owns the files of its display. A container that stops without
# this stop path, for example from a kill that skips it or from an out-of-memory
# kill, keeps the lock file and the socket in its writable layer, and the next
# start must not take them for a running server.
clear_stale_display() {
    local lock_file="/tmp/.X${DISPLAY_NUMBER}-lock"
    local socket_file="/tmp/.X11-unix/X${DISPLAY_NUMBER}"
    local lock_owner=""
    if [[ ! -e "$lock_file" && ! -e "$socket_file" ]]; then
        return 0
    fi
    if [[ -f "$lock_file" ]]; then
        lock_owner="$(tr -cd '[:digit:]' <"$lock_file" || true)"
    fi
    if [[ -n "$lock_owner" ]] && kill -0 "$lock_owner" 2>/dev/null; then
        log "the lock file $lock_file names the running process $lock_owner"
        return 0
    fi
    rm -f "$lock_file" "$socket_file"
    log "removed the lock file and the socket of an earlier run of $DISPLAY"
}

# The log of Xvfb names the reason of a start that fails, for example a lock
# file that it cannot use.
xvfb_log="/tmp/.X${DISPLAY_NUMBER}-xvfb.log"
clear_stale_display
Xvfb "$DISPLAY" -screen 0 "$SCREEN" -nolisten tcp -ac >"$xvfb_log" 2>&1 &
xvfb_pid=$!
# The socket appears only after the removal above, so the socket of this wait
# belongs to this Xvfb.
display_ready=0
for _ in {1..100}; do
    if [[ -S "/tmp/.X11-unix/X${DISPLAY_NUMBER}" ]] && kill -0 "$xvfb_pid" 2>/dev/null; then
        display_ready=1
        break
    fi
    if ! kill -0 "$xvfb_pid" 2>/dev/null; then
        break
    fi
    sleep 0.1
done
if [[ "$display_ready" -ne 1 ]]; then
    if kill -0 "$xvfb_pid" 2>/dev/null; then
        log "the display $DISPLAY did not appear within 10 s"
    else
        log "the display $DISPLAY did not start, Xvfb stopped"
    fi
    while IFS= read -r line; do
        log "Xvfb: $line"
    done <"$xvfb_log"
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

# The SSH server of the local container. A published machine has none: its image
# target `browser-vm` holds no server, and this block needs the variable that the
# local start sets. The key of the person is the only key that opens it.
start_ssh() {
    if [[ "$SSH_ENABLED" == 0 ]]; then
        return 0
    fi
    if [[ "$SSH_ENABLED" != 1 ]]; then
        log "HORIZON_BROWSER_VM_SSH must be 1 or 0, not $SSH_ENABLED"
        exit 1
    fi
    # sshd refuses a bare command name: it must be started by its absolute path.
    local sshd_bin
    if ! sshd_bin="$(command -v sshd)"; then
        log "this image has no SSH server; build the target browser-vm-local for a local container"
        exit 1
    fi
    if [[ -z "$SSH_PUBKEY" ]]; then
        log "HORIZON_BROWSER_VM_SSH=1 needs HORIZON_BROWSER_VM_SSH_PUBKEY with one public key"
        exit 1
    fi
    local ssh_dir="${STATE_DIR}/ssh"
    local host_key="${ssh_dir}/ssh_host_ed25519_key"
    local home_dir="${HOME:-/root}"
    mkdir -p "$ssh_dir" "$home_dir/.ssh" /run/sshd
    chmod 0700 "$ssh_dir" "$home_dir/.ssh"
    if [[ ! -f "$host_key" ]]; then
        ssh-keygen -q -t ed25519 -N '' -C horizon-browser-vm-local -f "$host_key"
        log "wrote the SSH host key to $host_key"
    fi
    printf '%s\n' "$SSH_PUBKEY" >"$home_dir/.ssh/authorized_keys"
    chmod 0600 "$home_dir/.ssh/authorized_keys"
    # A session of `ssh -T` gets no login shell, so the root of the browser
    # runtime and the display reach it through the permitted user environment.
    #
    # The staging root is the one folder that a bridge copies an attachment into
    # before it names the copy to the profile host. Without it here, the host
    # allows only the home of root and refuses every upload with
    # `attachment_policy`.
    printf 'HORIZON_BROWSER_ROOT=%s\nDISPLAY=%s\nHORIZON_BROWSER_ATTACHMENT_ROOTS=%s\n' \
        "$HORIZON_BROWSER_ROOT" "$DISPLAY" "${HORIZON_BROWSER_ROOT}/browser-staging" \
        >"$home_dir/.ssh/environment"
    chmod 0600 "$home_dir/.ssh/environment"
    local config="${ssh_dir}/sshd_config"
    cat >"$config" <<EOF
Port ${SSH_PORT}
ListenAddress 0.0.0.0
HostKey ${host_key}
PidFile /run/sshd.pid
AuthorizedKeysFile .ssh/authorized_keys
PermitRootLogin prohibit-password
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitEmptyPasswords no
UsePAM no
PermitUserEnvironment yes
LogLevel INFO
EOF
    chmod 0600 "$config"
    "$sshd_bin" -f "$config" -D -e &
    sshd_pid=$!
    local ready=0
    for _ in {1..100}; do
        if ! kill -0 "$sshd_pid" 2>/dev/null; then
            break
        fi
        if (exec 3<>"/dev/tcp/127.0.0.1/${SSH_PORT}") 2>/dev/null; then
            exec 3>&- 3<&-
            ready=1
            break
        fi
        sleep 0.1
    done
    if [[ "$ready" -ne 1 ]]; then
        log "the SSH server did not answer on the container port $SSH_PORT"
        exit 1
    fi
    log "the SSH server listens on the container port $SSH_PORT"
}

start_ssh

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
