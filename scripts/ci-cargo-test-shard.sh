#!/usr/bin/env bash
# One platform test shard. On Linux, ui, libs, libs-extra, and speech run as
# separate jobs and share the OS cache entry. Only the speech job saves that
# entry, and only on main. A cold main entry is filled by building the whole
# workspace in that job before the speech feature, so the saved target is not
# speech-only.
#
# `libs` runs the one workspace selection that carries the opt-in features with
# `cargo nextest`, which schedules the tests of every binary in one pool, and
# adds the doctests that nextest cannot run. `libs-extra` runs the single-package
# lanes and the example, which keep libtest. They are two jobs because cargo
# resolves features per selection: one job that ran both paid for both graphs in
# series, and the workspace selection alone was 656 s of its 999 s.
#
# macOS runs libs, libs-extra, and speech as three jobs over two cache keys:
# libs-extra restores the entry that libs saves and never writes it. libs and
# speech keep one key each because the jobs build different graphs, so one
# shared entry would leave one of them to rebuild what the other saved. This
# shard set skips the ui shard: speech runs the same horizon-ui tests with the
# feature on, and the Linux ui job still tests the build without it.
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

# The opt-in features that change no behaviour of another lane, so one build
# covers each of those lanes. exposure-gate is not here: two
# trusted_tcp_producer tests fail when it is on, so its lane keeps its build.
# exposure-firewall is here for the feature build; the default build of
# horizon-cloud also keeps its own run, because one test asserts the behaviour
# that the feature changes.
libs_features() {
    printf '%s' "horizon-cloud/exposure-firewall,horizon-core/one-service-gateway,horizon-core/cloud-exposure,horizon-chromecast/encoder"
}

# Each step returns its own status instead of aborting, so one failing command
# still reports the checks after it.
run_libs() {
    local status=0 features stream
    features="$(libs_features)"
    stream="${RUNNER_TEMP:-/tmp}/nextest-libs.json"
    # One build for the gateway and encoder lanes, plus the exposure-firewall
    # lane of horizon-cloud. Their own builds used to rebuild horizon-core and
    # its dependents for each lane, which cost more than the tests themselves.
    # Repeat this package selection and this feature list in every invocation of
    # this shard: cargo resolves features per selection, so a `-p` selection
    # after a `--workspace` build rebuilds every crate.
    # `cargo test` runs one test binary at a time, so 66 binaries ran in series
    # and the shard's 236 s of test time was a sum. nextest schedules the tests
    # of every binary in one pool, so the same tests overlap instead.
    NEXTEST_EXPERIMENTAL_LIBTEST_JSON=1 \
        run_cargo_kept "$stream" nextest run --locked --workspace --exclude horizon-ui \
        --features "$features" --message-format libtest-json-plus || status=$?
    # nextest runs no doctests, so they keep their own pass over the same units.
    run_cargo test --locked --workspace --exclude horizon-ui --features "$features" --doc || status=$?
    # A test over its allowance fails the shard that ran it, not a later sweep.
    run_duration_gate "$stream" || status=$?
    return "$status"
}

# The lanes that cannot join the workspace selection above, because each one
# asserts behaviour that another feature changes. They are seconds of tests over
# their own builds, so they run beside `libs` on a second runner instead of
# after it.
run_libs_extra() {
    local status=0
    # The shipped configuration first, with no opt-in feature. One horizon-cloud
    # test asserts that the default build accepts an unknown firewall attachment
    # field, and that assertion cannot run in the feature build.
    run_cargo test --locked -p horizon-cloud || status=$?
    # The original Unix/FD producer is a default-off Linux Worker component. Its
    # two lanes keep their own builds, because exposure-gate changes the
    # behaviour that the trusted_tcp_producer tests assert.
    if [[ "${RUNNER_OS:?RUNNER_OS is required}" == "Linux" ]]; then
        run_cargo test --locked -p horizon-cloud-worker --features trusted-tcp-producer trusted_tcp_producer:: || status=$?
        run_cargo test --locked -p horizon-cloud-worker --features trusted-exposure-gate exposure_gate:: || status=$?
    fi
    # Selection and inventory tests run through the example binary.
    run_cargo test --locked -p horizon-core --example cloud_deploy -- tailnets:: tests::malformed_selection_arguments_never_create_cloud_paths || status=$?
    return "$status"
}

run_speech() {
    run_cargo test --locked -p horizon-ui --features speech
}

case "$shard" in
    ui)
        run_ui
        ;;
    libs)
        run_libs
        ;;
    libs-extra)
        run_libs_extra
        ;;
    speech)
        # The Linux speech job saves `linux-test`, so only it must fill a cold
        # one with the whole workspace. A macOS speech job saves only its own
        # speech graph, so the workspace build there would be thrown away.
        if [[ "${RUNNER_OS:?RUNNER_OS is required}" == "Linux" &&
              "${GITHUB_REF:-}" == "refs/heads/main" &&
              "${HORIZON_CI_CACHE_HIT:-}" != "true" ]]; then
            run_cargo test --locked --workspace --no-run
        fi
        run_speech
        ;;
    *)
        printf 'Unknown CI test shard: %s\n' "$shard" >&2
        exit 1
        ;;
esac
