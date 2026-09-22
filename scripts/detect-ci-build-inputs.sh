#!/usr/bin/env bash
# Classify the paths changed between two commits for the CI lanes that are
# gated on them: the Snap package and the release Artifacts matrix.
#
# Usage: detect-ci-build-inputs.sh <event-name> <base-sha> <head-sha>
#
# Prints `run_snap_build=<bool>` and `release_inputs=<bool>`, in the
# `key=value` form a workflow step appends to $GITHUB_OUTPUT. Runs in the
# current working directory, which must be the repository checkout.
set -euo pipefail

if [ "$#" -ne 3 ]; then
  echo "usage: $(basename "$0") <event-name> <base-sha> <head-sha>" >&2
  exit 2
fi

event_name=$1
base_sha=$2
head_sha=$3

emit() {
  printf 'run_snap_build=%s\nrelease_inputs=%s\n' "$1" "$2"
}

# A manual dispatch, a branch's first push (an all-zero base) and a base this
# checkout cannot resolve all leave the diff unknown, so they build everything
# rather than silently skipping a package or a release.
if [ "$event_name" = "workflow_dispatch" ] \
  || ! git rev-parse --verify --quiet "${base_sha}^{commit}" > /dev/null 2>&1 \
  || ! changed_paths=$(git diff --name-only "$base_sha" "$head_sha" 2>/dev/null); then
  emit true true
  exit 0
fi

run_snap_build=false
release_inputs=false

while IFS= read -r path; do
  case "$path" in
    .github/workflows/ci.yml|.github/workflows/release.yml|scripts/build-surge-toolchain.sh|scripts/package-release-asset.sh|scripts/stage-surge-artifacts.sh|snap/*|packaging/linux/*|assets/icons/*)
      run_snap_build=true
      ;;
  esac
  case "$path" in
    .github/workflows/ci.yml|scripts/package-release-asset.sh|rust-toolchain.toml|Cargo.toml|Cargo.lock|assets/*|crates/*)
      release_inputs=true
      ;;
  esac
done <<< "$changed_paths"

emit "$run_snap_build" "$release_inputs"
