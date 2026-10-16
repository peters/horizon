#!/usr/bin/env bash
# Classify the paths changed between two commits for the CI lanes that are
# gated on them: the Snap package, the release Artifacts builds and, on pull
# requests, the Rust build and test jobs and the Linux speech tests.
#
# Usage: detect-ci-build-inputs.sh <event-name> <base-sha> <head-sha>
#
# Prints `run_snap_build=<bool>`, `release_inputs=<bool>`,
# `macos_release_inputs=<bool>`, `rust_inputs=<bool>` and
# `speech_inputs=<bool>`, in the `key=value` form a workflow step appends to
# $GITHUB_OUTPUT. Runs in the current working directory, which must be the
# repository checkout.
#
# `release_inputs` gates the Linux and Windows release builds on main.
# `macos_release_inputs` is the narrower set that also builds the two macOS
# release assets at once instead of waiting for the nightly macOS release
# builds: the files that define how a release asset is built and packaged.
# Source, manifest, lock file and toolchain changes wait for the nightly run:
# Dependabot changes Cargo.lock on most days, and the macOS test job on every
# push still compiles those changes on macOS. Every path in it is also a
# release input, so main never builds macOS without Linux and Windows.
#
# `rust_inputs` is false only for a pull request whose every changed path is
# on the short list below of files no Rust build or test reads. Any other
# path, every push and every manual dispatch keep it true, so main always
# runs the whole matrix and keeps the build caches warm.
#
# `speech_inputs` is false only for a pull request that changes nothing the
# Linux speech test job reads beyond what the other jobs test: no file that
# names the speech feature (`feature = "speech..."` or CARGO_FEATURE_SPEECH)
# before or after the change, nothing in a speech, dictation or transcribe
# module, no Cargo manifest, lock file, Cargo configuration or toolchain, and
# not the workflow or scripts that run that job. The vendored speech C++ tree
# comes from the transcribe-cpp crate, so it changes only with a manifest or
# the lock file. Every push and dispatch keep it true, because the speech job
# on main saves the Linux test cache.
set -euo pipefail

if [ "$#" -ne 3 ]; then
  echo "usage: $(basename "$0") <event-name> <base-sha> <head-sha>" >&2
  exit 2
fi

event_name=$1
base_sha=$2
head_sha=$3

emit() {
  printf 'run_snap_build=%s\nrelease_inputs=%s\nmacos_release_inputs=%s\nrust_inputs=%s\nspeech_inputs=%s\n' \
    "$1" "$2" "$3" "$4" "$5"
}

# Prints the files under crates/ that name the speech feature in the base or
# the head tree, one per line. Fails when git cannot search a tree.
speech_gated_files() {
  local found status=0
  found=$(git grep -l -z -E 'feature[[:space:]]*=[[:space:]]*"speech|CARGO_FEATURE_SPEECH' \
    "$base_sha" "$head_sha" -- crates 2>/dev/null | tr '\0' '\n') || status=$?
  # 1 means no match.
  if [ "$status" -gt 1 ]; then
    return 1
  fi
  # Each match is `<tree>:<path>`, and neither tree name contains a colon.
  [ -z "$found" ] || printf '%s\n' "$found" | cut -d: -f2- | sort -u
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
  emit true true true true true
  exit 0
fi

run_snap_build=false
release_inputs=false
macos_release_inputs=false
rust_inputs=false
speech_inputs=false
declare -A speech_gated=()
if [ "$event_name" != "pull_request" ]; then
  rust_inputs=true
  speech_inputs=true
elif gated=$(speech_gated_files); then
  while IFS= read -r path; do
    [ -z "$path" ] || speech_gated[$path]=1
  done <<< "$gated"
else
  speech_inputs=true
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
  case "$path" in
    crates/*speech*|crates/*dictat*|crates/*transcrib*|crates/horizon-ui/build.rs \
      |Cargo.toml|*/Cargo.toml|Cargo.lock|*/Cargo.lock|rust-toolchain.toml|.cargo/* \
      |.github/workflows/ci.yml|scripts/ci-cargo-test-shard.sh|scripts/install-ci-ubuntu-dependencies.sh)
      speech_inputs=true
      ;;
  esac
  if [ -n "$path" ] && [ -n "${speech_gated[$path]:-}" ]; then
    speech_inputs=true
  fi
done <<< "$changed_paths"

emit "$run_snap_build" "$release_inputs" "$macos_release_inputs" "$rust_inputs" "$speech_inputs"
