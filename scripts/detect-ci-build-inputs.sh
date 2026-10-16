#!/usr/bin/env bash
# Classify the paths changed between two commits for the CI lanes that are
# gated on them: the Snap package, the release Artifacts builds and, on pull
# requests, the Rust build and test jobs.
#
# Usage: detect-ci-build-inputs.sh <event-name> <base-sha> <head-sha>
#
# Prints `run_snap_build=<bool>`, `release_inputs=<bool>`,
# `macos_release_inputs=<bool>` and `rust_inputs=<bool>`, in the `key=value`
# form a workflow step appends to $GITHUB_OUTPUT. Runs in the current working
# directory, which must be the repository checkout.
#
# `release_inputs` gates the Linux and Windows release builds on main.
# `macos_release_inputs` is the narrower set that also builds the two macOS
# release assets at once instead of waiting for the nightly macOS release
# builds: the files that define how a release asset is built and packaged.
# A Rust source change alone waits for the nightly run. Every path in it is
# also a release input, so main never builds macOS without Linux and Windows.
#
# `rust_inputs` is false only for a pull request whose every changed path is
# on the short list below of files no Rust build or test reads. Any other
# path, every push and every manual dispatch keep it true, so main always
# runs the whole matrix and keeps the build caches warm.
set -euo pipefail

if [ "$#" -ne 3 ]; then
  echo "usage: $(basename "$0") <event-name> <base-sha> <head-sha>" >&2
  exit 2
fi

event_name=$1
base_sha=$2
head_sha=$3

emit() {
  printf 'run_snap_build=%s\nrelease_inputs=%s\nmacos_release_inputs=%s\nrust_inputs=%s\n' \
    "$1" "$2" "$3" "$4"
}

# Paths that no Rust build or test reads. docs/architecture/*.json and
# docs/features/ stay out: horizon-app-testing reads them in its tests.
outside_rust() {
  case "$1" in
    .github/workflows/ci.yml) return 1 ;;
    docs/architecture/*.json|docs/features/*) return 1 ;;
    .github/*|docs/*|.claude/*) return 0 ;;
    */*) return 1 ;;
    *.md) return 0 ;;
  esac
  return 1
}

# `--no-renames` lists both sides of a rename, so moving a source file into
# docs/ still counts as a change to the source. `-z` keeps every path
# unquoted, so a path with a quote or a non-ASCII name still matches its
# folder's pattern.
# A manual dispatch, a branch's first push (an all-zero base) and a base this
# checkout cannot resolve all leave the diff unknown, so they build everything
# rather than silently skipping a package or a release.
if [ "$event_name" = "workflow_dispatch" ] \
  || ! git rev-parse --verify --quiet "${base_sha}^{commit}" > /dev/null 2>&1 \
  || ! changed_paths=$(git diff -z --name-only --no-renames "$base_sha" "$head_sha" 2>/dev/null | tr '\0' '\n'); then
  emit true true true true
  exit 0
fi

run_snap_build=false
release_inputs=false
macos_release_inputs=false
rust_inputs=false
if [ "$event_name" != "pull_request" ]; then
  rust_inputs=true
fi

while IFS= read -r path; do
  case "$path" in
    .github/workflows/ci.yml|.github/workflows/release.yml|scripts/package-release-asset.sh|snap/*|packaging/linux/*|assets/icons/*)
      run_snap_build=true
      ;;
  esac
  case "$path" in
    .github/workflows/release-artifacts.yml|scripts/select-release-artifact-platforms.py|scripts/package-release-asset.sh|scripts/install-ci-video-build-tools.sh)
      macos_release_inputs=true
      release_inputs=true
      ;;
    .github/workflows/ci.yml|rust-toolchain.toml|Cargo.toml|Cargo.lock|assets/*|crates/*)
      release_inputs=true
      ;;
  esac
  if [ -n "$path" ] && ! outside_rust "$path"; then
    rust_inputs=true
  fi
done <<< "$changed_paths"

emit "$run_snap_build" "$release_inputs" "$macos_release_inputs" "$rust_inputs"
