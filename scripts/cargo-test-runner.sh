#!/bin/sh
# Runs one binary that cargo started.
#
# Cargo starts every binary of `cargo run` and `cargo test` through this script,
# and `cargo nextest` reads the same setting from `.cargo/config.toml`. A test
# binary lives in `target/<profile>/deps/`, and the application of this workspace
# never does, so only a test process runs with a home of its own in the temporary
# folder: no test reads or writes `~/.horizon`, `~/.codex` or a profile folder of
# the person. The value of `HORIZON_FORBID_REAL_HOME` is that home, and the guard
# of `horizon-browser-control` ends a test that names it anyway, with the line
# that asked for it.
case "$1" in
  */deps/*)
    person=${HOME:-}
    private=$(mktemp -d "${TMPDIR:-/var/tmp}/horizon-test-home.XXXXXX") || exit 1
    if [ -n "$person" ]; then
      HORIZON_FORBID_REAL_HOME="$person"
      export HORIZON_FORBID_REAL_HOME
    fi
    HOME="$private"
    export HOME
    "$@"
    status=$?
    rm -rf "$private"
    exit "$status"
    ;;
esac
exec "$@"
