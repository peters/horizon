#!/usr/bin/env bash
# Test only a disposable Secret Service daemon, never the desktop credential store.
set -euo pipefail
if [[ "$(uname -s)" != "Linux" ]]; then
    printf '%s\n' 'This private Secret Service fixture requires Linux.' >&2
    exit 2
fi
cd "$(dirname "$0")/../.."
if [[ "${1:-}" != "--private-bus" ]]; then
    for command in dbus-run-session gnome-keyring-daemon gdbus cargo; do
        command -v "$command" >/dev/null
    done
    fixture_root=$(mktemp -d /tmp/horizon-owner-keyring.XXXXXX)
    trap 'rm -rf -- "$fixture_root"' EXIT
    mkdir -m 700 "$fixture_root/data" "$fixture_root/runtime" "$fixture_root/control"
    env -u DISPLAY -u WAYLAND_DISPLAY -u GNOME_KEYRING_CONTROL -u SSH_AUTH_SOCK \
        XDG_DATA_HOME="$fixture_root/data" XDG_RUNTIME_DIR="$fixture_root/runtime" \
        HORIZON_OWNER_TEST_ROOT="$fixture_root" \
        dbus-run-session -- bash "$0" --private-bus
    exit
fi
: "${HORIZON_OWNER_TEST_ROOT:?private fixture root required}"
: "${DBUS_SESSION_BUS_ADDRESS:?private bus required}"
export HORIZON_OWNER_TEST_BUS="$DBUS_SESSION_BUS_ADDRESS"
fixture_daemon_pid=
stop_fixture() {
    if [[ -n "$fixture_daemon_pid" ]]; then
        kill "$fixture_daemon_pid" 2>/dev/null || true
        wait "$fixture_daemon_pid" 2>/dev/null || true
        fixture_daemon_pid=
    fi
}
trap stop_fixture EXIT
start_fixture() {
    printf '%s' 'synthetic-fixture-password' | \
        gnome-keyring-daemon --foreground --unlock --components=secrets \
            --control-directory="$HORIZON_OWNER_TEST_ROOT/control" \
            >"$HORIZON_OWNER_TEST_ROOT/daemon.log" 2>&1 &
    fixture_daemon_pid=$!
    gdbus wait --session --timeout 15 org.freedesktop.secrets
    local service_pid
    service_pid=$(gdbus call --session --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus \
        --method org.freedesktop.DBus.GetConnectionUnixProcessID org.freedesktop.secrets)
    [[ "$service_pid" == "(uint32 $fixture_daemon_pid,)" ]]
}
run_phase() {
    local phase_output
    if ! phase_output=$(HORIZON_OWNER_TEST_PHASE="$1" CARGO_TERM_COLOR=never cargo test --locked -p horizon-core --lib \
        cloud_runtime::owner::tests::native_store_fixture -- --ignored --exact --format pretty 2>&1); then
        printf '%s\n' "$phase_output" >&2
        return 1
    fi
    printf '%s\n' "$phase_output"
    [[ "$phase_output" == *'test cloud_runtime::owner::tests::native_store_fixture ... ok'* ]]
}
start_fixture
run_phase create
kill -KILL "$fixture_daemon_pid"
wait "$fixture_daemon_pid" 2>/dev/null || true
fixture_daemon_pid=
start_fixture
run_phase reopen
stop_fixture
start_fixture
run_phase reopen
printf '%s\n' 'Private native credential-store crash, restart and signing checks passed.'
