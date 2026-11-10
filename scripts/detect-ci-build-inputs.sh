#!/usr/bin/env bash
# Classify the paths changed between two commits for the CI lanes that are
# gated on them: the release Artifacts builds and, on pull requests, the Rust
# build and test jobs.
#
# Usage: detect-ci-build-inputs.sh <event-name> <base-sha> <head-sha>
#
# Prints `release_inputs=<bool>`, `macos_release_inputs=<bool>`,
# `rust_inputs=<bool>` and `vnc_pin=<bool>` in the `key=value` form a workflow
# step appends to $GITHUB_OUTPUT. Runs in the current working directory, which
# must be the repository checkout.
#
# `release_inputs` gates the Linux release build on main. The workflow files
# that run a release are inputs too, so a change to a lane builds an artifact
# before the next tag uses it.
# `macos_release_inputs` is the narrower set that also builds the two macOS
# release assets at once instead of waiting for the nightly macOS release
# builds: the files that define how a release asset is built and packaged.
# Source, manifest, lock file and toolchain changes wait for the nightly run:
# Dependabot changes Cargo.lock on most days, and the macOS test job on every
# push still compiles those changes on macOS. Every path in it is also a
# release input, so main never builds macOS without Linux.
#
# `rust_inputs` is false only for a pull request whose every changed path is
# on the short list below of files no Rust build or test reads. Any other
# path, every push and every manual dispatch keep it true, so main always
# runs the whole matrix and keeps the build caches warm.
#
# `vnc_pin` is true when the `vnc` decoder rev in Cargo.toml differs from the
# base, which is the input that runs the decoder's own formatting, tests and
# Clippy. The rev names an upstream commit that cannot change under it, so a run
# without a new rev repeats a result that nobody can invalidate, and a run with one
# is the first to see it. Removing the dependency leaves no pin to qualify, so the
# step skips. The step and this script are inputs too: a change to either runs the
# qualification once, so a broken step fails on its own change instead of on a
# later one that moves the rev.
set -euo pipefail

if [ "$#" -ne 3 ]; then
  echo "usage: $(basename "$0") <event-name> <base-sha> <head-sha>" >&2
  exit 2
fi

event_name=$1
base_sha=$2
head_sha=$3

emit() {
  printf 'release_inputs=%s\nmacos_release_inputs=%s\nrust_inputs=%s\nvnc_pin=%s\n' \
    "$1" "$2" "$3" "$4"
}

# The `vnc` decoder rev in the Cargo.toml of one tree. Prints nothing when that
# tree has no manifest or no such dependency, and `?` when a manifest is there but
# the pin cannot be read, so an unreadable pin is qualified rather than skipped.
vnc_rev_at() {
  # A tree without the manifest prints nothing; `|| true` keeps `pipefail` and
  # `set -e` from treating that as a failed detection.
  { git show "$1:Cargo.toml" 2>/dev/null || true; } | python3 -c '
import sys, tomllib
text = sys.stdin.read()
if not text.strip():
    raise SystemExit
try:
    manifest = tomllib.loads(text)
except tomllib.TOMLDecodeError:
    print("?")
    raise SystemExit
pinned = manifest.get("workspace", {}).get("dependencies", {}).get("vnc")
print(pinned.get("rev", "?") if isinstance(pinned, dict) else "")
'
}

# Paths that no Rust build or test reads.
outside_rust() {
  case "$1" in
    .github/workflows/ci.yml) return 1 ;;
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
# rather than silently skipping a release build or a Rust job.
if [ "$event_name" = "workflow_dispatch" ] \
  || ! git rev-parse --verify --quiet "${base_sha}^{commit}" > /dev/null 2>&1 \
  || ! changed_paths=$(git diff -z --name-only --no-renames "$base_sha" "$head_sha" 2>/dev/null | tr '\0' '\n'); then
  emit true true true true
  exit 0
fi

release_inputs=false
macos_release_inputs=false
rust_inputs=false
vnc_step_changed=false
if [ "$event_name" != "pull_request" ]; then
  rust_inputs=true
fi

while IFS= read -r path; do
  case "$path" in
    .github/workflows/release-artifacts.yml|scripts/select-release-artifact-platforms.py|scripts/package-release-asset.sh|scripts/install-ci-video-build-tools.sh)
      macos_release_inputs=true
      release_inputs=true
      ;;
    .github/workflows/ci.yml|.github/workflows/release.yml|rust-toolchain.toml|Cargo.toml|Cargo.lock|assets/*|crates/*)
      release_inputs=true
      ;;
  esac
  if [ -n "$path" ] && ! outside_rust "$path"; then
    rust_inputs=true
  fi
  case "$path" in
    .github/workflows/ci.yml|scripts/detect-ci-build-inputs.sh)
      vnc_step_changed=true
      ;;
  esac
done <<< "$changed_paths"

head_vnc_pin=$(vnc_rev_at "$head_sha")
vnc_pin=$vnc_step_changed
if [ -n "$head_vnc_pin" ] && [ "$head_vnc_pin" != "$(vnc_rev_at "$base_sha")" ]; then
  vnc_pin=true
fi

emit "$release_inputs" "$macos_release_inputs" "$rust_inputs" "$vnc_pin"
