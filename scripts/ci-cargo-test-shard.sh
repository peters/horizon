#!/usr/bin/env bash
# One platform test shard. ui, libs, and speech run as separate jobs and share
# the OS cache entry. Only the speech job saves that entry, and only on main.
# A cold main entry is filled by building the whole workspace in that job
# before the speech feature, so the saved target is not speech-only.
set -euo pipefail

shard="${HORIZON_CI_SHARD:?HORIZON_CI_SHARD is required}"

run_cargo() {
    if [[ "${HORIZON_CI_SHARD_DRY_RUN:-}" == "1" ]]; then
        printf '%s\n' "cargo $*"
        return 0
    fi
    cargo "$@"
}

case "$shard" in
    ui)
        run_cargo test --locked -p horizon-ui
        ;;
    libs)
        run_cargo test --locked --workspace --exclude horizon-ui
        # The shared-encoder sink is opt-in, so the workspace run does not build it.
        run_cargo test --locked -p horizon-chromecast --features encoder
        ;;
    speech)
        if [[ "${GITHUB_REF:-}" == "refs/heads/main" && "${HORIZON_CI_CACHE_HIT:-}" != "true" ]]; then
            run_cargo test --locked --workspace --no-run
        fi
        if [[ "${RUNNER_OS:?RUNNER_OS is required}" == "Windows" ]]; then
            run_cargo test --locked -p horizon-ui --features speech --no-run
        else
            run_cargo test --locked -p horizon-ui --features speech
        fi
        ;;
    *)
        printf 'Unknown CI test shard: %s\n' "$shard" >&2
        exit 1
        ;;
esac
