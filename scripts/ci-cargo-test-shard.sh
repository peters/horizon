#!/usr/bin/env bash
# One platform test shard. ui and libs run on Linux and on macOS, so the
# workflow starts four jobs in total.
#
# `ui` runs the horizon-ui tests with `cargo test`.
#
# `libs` runs every other package of the workspace with `cargo nextest`, which
# schedules the tests of every binary in one pool instead of one binary at a
# time, and adds the doctests that nextest cannot run. Its libtest-json stream
# is kept for the duration gate and, on main, for the published durations.
#
# The two shards select different packages, and cargo resolves features per
# package selection, so each one keeps its own cache entry. A shared entry would
# be a full match that still rebuilds.
set -euo pipefail

shard="${HORIZON_CI_SHARD:?HORIZON_CI_SHARD is required}"

run_cargo() {
    if [[ "${HORIZON_CI_SHARD_DRY_RUN:-}" == "1" ]]; then
        printf '%s\n' "cargo $*"
        return 0
    fi
    cargo "$@"
}

# Runs cargo and keeps its standard output in a file. nextest writes its
# libtest-json stream there and its human report to standard error, so the log
# keeps the report while the duration gate reads the stream.
run_cargo_kept() {
    local stream=$1
    shift
    if [[ "${HORIZON_CI_SHARD_DRY_RUN:-}" == "1" ]]; then
        printf '%s\n' "cargo $* > $stream"
        return 0
    fi
    cargo "$@" > "$stream"
}

# The gate reads the stream that the run just wrote, so it adds no run time.
run_duration_gate() {
    local stream=$1
    if [[ "${HORIZON_CI_SHARD_DRY_RUN:-}" == "1" ]]; then
        printf '%s\n' "python3 -B scripts/check-test-durations.py --stream $stream"
        return 0
    fi
    python3 -B scripts/check-test-durations.py --stream "$stream"
}

run_ui() {
    run_cargo test --locked -p horizon-ui
}

# Each step returns its own status instead of aborting, so one failing command
# still reports the checks after it.
run_libs() {
    local status=0 stream
    stream="${RUNNER_TEMP:-/tmp}/nextest-libs.json"
    # `cargo test` runs one test binary at a time, so a shard sums the test time
    # of its binaries. nextest schedules the tests of every binary in one pool,
    # so the same tests overlap instead.
    NEXTEST_EXPERIMENTAL_LIBTEST_JSON=1 \
        run_cargo_kept "$stream" nextest run --locked --workspace --exclude horizon-ui \
        --message-format libtest-json-plus || status=$?
    # nextest runs no doctests, so they keep their own pass over the same units.
    run_cargo test --locked --workspace --exclude horizon-ui --doc || status=$?
    # A test over its allowance fails the shard that ran it, not a later sweep.
    run_duration_gate "$stream" || status=$?
    return "$status"
}

case "$shard" in
    ui)
        run_ui
        ;;
    libs)
        run_libs
        ;;
    *)
        printf 'Unknown CI test shard: %s\n' "$shard" >&2
        exit 1
        ;;
esac
