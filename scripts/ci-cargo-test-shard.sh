#!/usr/bin/env bash
# One platform test shard. On Linux, ui, libs, and speech run as separate
# jobs and share the OS cache entry. Only the speech job saves that entry, and
# only on main. A cold main entry is filled by building the whole workspace in
# that job before the speech feature, so the saved target is not speech-only.
#
# macOS runs libs and speech as two jobs. The libs job saves the entry, because
# it builds most of the workspace. This shard set skips the ui shard: speech
# runs the same horizon-ui tests with the feature on, and the Linux ui job
# still tests the build without it.
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
# exposure-firewall is here for the feature build; the default build of
# horizon-cloud also keeps its own run, because one test asserts the behaviour
# that the feature changes.
libs_features() {
    printf '%s' "horizon-cloud/exposure-firewall,horizon-core/one-service-gateway,horizon-core/cloud-exposure,horizon-chromecast/encoder"
}

# Each step returns its own status instead of aborting, so one failing command
# still reports the checks after it.
run_libs() {
    local status=0 features
    features="$(libs_features)"
    # The shipped configuration first, with no opt-in feature. One horizon-cloud
    # test asserts that the default build accepts an unknown firewall attachment
    # field, and that assertion cannot run in the feature build below.
    run_cargo test --locked -p horizon-cloud || status=$?
    # One build for the gateway and encoder lanes, plus the exposure-firewall
    # lane of horizon-cloud. Their own builds used to rebuild horizon-core and
    # its dependents for each lane, which cost more than the tests themselves.
    # Every later invocation repeats this package selection and this feature
    # list: cargo resolves features per selection, so a `-p` selection after a
    # `--workspace` build rebuilds every crate.
    run_cargo test --locked --workspace --exclude horizon-ui --no-run --features "$features" || status=$?
    run_cargo test --locked --workspace --exclude horizon-ui --features "$features" || status=$?
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
    speech)
        # Only the Linux speech job saves the entry, so only it must fill a cold
        # one with the whole workspace. The macOS job saves no entry, so that
        # build would be thrown away.
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
