#!/usr/bin/env bash
# One platform test shard. On Linux and Windows, ui, libs, and speech run as
# separate jobs and share the OS cache entry. Only the speech job saves that
# entry, and only on main. A cold main entry is filled by building the whole
# workspace in that job before the speech feature, so the saved target is not
# speech-only.
#
# macOS runs `all`: the three shards in sequence on one runner. GitHub-hosted
# accounts get about five concurrent macOS jobs, so three macOS jobs per run
# queued behind each other whenever a few pull requests ran at once.
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

# Each step returns its own status: `all` calls these with `||`, which turns
# off `set -e` inside them.
run_libs() {
    local status=0
    run_cargo test --locked --workspace --exclude horizon-ui || status=$?
    # The shared-encoder sink is opt-in, so the workspace run does not build it.
    run_cargo test --locked -p horizon-chromecast --features encoder || status=$?
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
        # libs and ui together build the whole workspace, so a cold main entry
        # needs no separate build step before speech. Each shard runs even
        # when an earlier one fails, so one job still reports all three.
        failed=()
        run_libs || failed+=(libs)
        run_ui || failed+=(ui)
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
