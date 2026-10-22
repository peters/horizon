#!/usr/bin/env bash
# One platform test shard. On Linux and Windows, ui, libs, and speech run as
# separate jobs and share the OS cache entry. Only the speech job saves that
# entry, and only on main. A cold main entry is filled by building the whole
# workspace in that job before the speech feature, so the saved target is not
# speech-only.
#
# macOS runs `all`: libs, then speech, on one runner. GitHub-hosted accounts
# get about five concurrent macOS jobs, so three macOS jobs per run queued
# behind each other whenever a few pull requests ran at once. `all` skips the
# ui shard: speech runs the same horizon-ui tests with the feature on, and the
# Linux ui job still tests the build without it.
set -euo pipefail

shard="${HORIZON_CI_SHARD:?HORIZON_CI_SHARD is required}"

run_cargo() {
    if [[ "${HORIZON_CI_SHARD_DRY_RUN:-}" == "1" ]]; then
        printf '%s\n' "cargo $*"
        return 0
    fi
    cargo "$@"
}

run_ui() {
    run_cargo test --locked -p horizon-ui
}

# The opt-in features that change no behaviour of another lane, so one build
# covers each of those lanes. exposure-gate is not here: two
# trusted_tcp_producer tests fail when it is on, so its lane keeps its build.
libs_features() {
    printf '%s' "horizon-cloud/exposure-firewall,horizon-core/one-service-gateway,horizon-core/cloud-exposure,horizon-chromecast/encoder"
}

# Each step returns its own status: `all` calls these with `||`, which turns
# off `set -e` inside them.
run_libs() {
    local status=0 features
    features="$(libs_features)"
    # One build for the cloud, gateway and encoder lanes. Their own builds used
    # to rebuild horizon-core and its dependents for each lane, which cost more
    # than the tests themselves. Every later invocation repeats this package
    # selection and this feature list: cargo resolves features per selection, so
    # a `-p` selection after a `--workspace` build rebuilds every crate.
    run_cargo test --locked --workspace --exclude horizon-ui --no-run --features "$features" || status=$?
    # This run covers the exposure-firewall, gateway, owner-custody and encoder
    # tests, which each had a lane of their own.
    run_cargo test --locked --workspace --exclude horizon-ui --features "$features" || status=$?
    # The original Unix/FD producer is a default-off Linux Worker component. Its
    # two lanes keep their own builds, because exposure-gate changes the
    # behaviour that the trusted_tcp_producer tests assert.
    if [[ "${RUNNER_OS:?RUNNER_OS is required}" == "Linux" ]]; then
        run_cargo test --locked -p horizon-cloud-worker --features trusted-tcp-producer trusted_tcp_producer:: || status=$?
        run_cargo test --locked -p horizon-cloud-worker --features trusted-exposure-gate exposure_gate:: || status=$?
    fi
    # Selection and inventory tests also cover the unsupported Windows state path.
    run_cargo test --locked -p horizon-core --example cloud_deploy -- tailnets:: tests::malformed_selection_arguments_never_create_cloud_paths || status=$?
    return "$status"
}

run_speech() {
    if [[ "${RUNNER_OS:?RUNNER_OS is required}" == "Windows" ]]; then
        run_cargo test --locked -p horizon-ui --features speech --no-run
    else
        run_cargo test --locked -p horizon-ui --features speech
    fi
}

case "$shard" in
    ui)
        run_ui
        ;;
    libs)
        run_libs
        ;;
    speech)
        if [[ "${GITHUB_REF:-}" == "refs/heads/main" && "${HORIZON_CI_CACHE_HIT:-}" != "true" ]]; then
            run_cargo test --locked --workspace --no-run
        fi
        run_speech
        ;;
    all)
        # libs and speech together build the whole workspace, so a cold main
        # entry needs no separate build step. speech runs even when libs
        # fails, so one job still reports both.
        failed=()
        run_libs || failed+=(libs)
        run_speech || failed+=(speech)
        if ((${#failed[@]})); then
            printf 'Failed CI test shards: %s\n' "${failed[*]}" >&2
            exit 1
        fi
        ;;
    *)
        printf 'Unknown CI test shard: %s\n' "$shard" >&2
        exit 1
        ;;
esac
