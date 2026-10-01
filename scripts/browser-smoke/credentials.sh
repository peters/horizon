#!/usr/bin/env bash
# No desktop secrets: the test owns a disposable bus, keyring and synthetic data.
set -euo pipefail
cd "$(dirname "$0")/../.."
if [[ "$(uname -s)" != "Linux" ]]; then
    printf '%s\n' 'This private Secret Service fixture requires Linux.' >&2
    exit 2
fi
if [[ "${1:-}" != "--private-bus" ]]; then
    for command in dbus-run-session gnome-keyring-daemon gdbus cargo; do
        command -v "$command" >/dev/null
    done
    fixture_root=$(mktemp -d /tmp/horizon-browser-keyring.XXXXXX)
    trap 'rm -rf -- "$fixture_root"' EXIT
    mkdir -m 700 "$fixture_root/data" "$fixture_root/runtime" "$fixture_root/control"
    env -u DISPLAY -u WAYLAND_DISPLAY -u GNOME_KEYRING_CONTROL -u SSH_AUTH_SOCK \
        XDG_DATA_HOME="$fixture_root/data" XDG_RUNTIME_DIR="$fixture_root/runtime" \
        HORIZON_CREDENTIAL_TEST_ROOT="$fixture_root" \
        dbus-run-session -- bash "$0" --private-bus
    exit
fi
: "${HORIZON_CREDENTIAL_TEST_ROOT:?private fixture root required}"
: "${DBUS_SESSION_BUS_ADDRESS:?private bus required}"
export HORIZON_CREDENTIAL_TEST_BUS="$DBUS_SESSION_BUS_ADDRESS"
cargo test --locked -p horizon-core --lib \
    remote_browser_credential::tests::native_keyring::credentials_survive_secret_service_restart \
    -- --ignored --exact
