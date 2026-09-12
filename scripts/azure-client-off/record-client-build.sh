#!/usr/bin/env bash
# Build the Horizon client for client VM A and record its provenance in one step.
#
# Run inside a fully clean checkout (no modified or untracked files) at the frozen
# candidate commit:
#   record-client-build.sh --out client-build.json [--target-dir <dir>]
# It captures HEAD, runs the release build of the `horizon` binary from that exact tree,
# hashes the binary it just produced and writes commit, digest and path together, so
# the digest can only belong to that commit. provision-client.sh verifies the record
# against the manifest and the bytes it uploads.
set -euo pipefail

fail() { printf 'record-client-build: %s\n' "$1" >&2; exit 1; }
out= target_dir=
while (($# > 0)); do
  case "$1" in
    --out) out=$2; shift 2 ;;
    --target-dir) target_dir=$2; shift 2 ;;
    *) fail "unknown argument: $1" ;;
  esac
done
[ -n "$out" ] || fail "--out is required"
for tool in git cargo jq sha256sum realpath; do command -v "$tool" >/dev/null 2>&1 || fail "required command not found: $tool"; done
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || fail "run this inside the checkout to build from"
toplevel=$(git rev-parse --show-toplevel)
target_dir=$(realpath -m "${target_dir:-$toplevel/target}")
# The build output is the only thing allowed to differ from the commit, and only at the
# checkout's own target/ or outside the checkout: anywhere else inside it, Cargo's
# output would read as an unclean tree after the build.
case "$target_dir" in
  "$toplevel"/target) ;;
  "$toplevel"/*) fail "--target-dir must be the checkout's target/ or lie outside the checkout" ;;
esac
# Modified, untracked and ignored files alike make the tree something other than the
# commit (the UI build prefers an ignored publish-assets directory when present); only
# the build output directory itself is exempt.
clean() { [ -z "$(git status --porcelain --ignored | grep -v -E '^!! target/$')" ]; }
clean || fail "the checkout is not clean (modified, untracked or ignored files besides target/); a frozen candidate is one commit"
sha=$(git rev-parse HEAD)
# Built for the client VM's platform explicitly, whatever the controller runs.
CARGO_TARGET_DIR=$target_dir cargo build --release -p horizon-ui --bin horizon --target x86_64-unknown-linux-gnu
binary=$target_dir/x86_64-unknown-linux-gnu/release/horizon
[ -f "$binary" ] || fail "the build produced no binary at $binary"
# The tree must still be the same commit after the build.
[ "$(git rev-parse HEAD)" = "$sha" ] && clean || fail "the checkout changed during the build"
# Client VM A is Ubuntu x86-64: only an ELF64 x86-64 binary can run there.
[ "$(od -An -tx1 -N4 "$binary" | tr -d ' \n')" = "7f454c46" ] || fail "the binary is not an ELF file; build the Linux x86-64 client"
[ "$(od -An -tx1 -j18 -N2 "$binary" | tr -d ' \n')" = "3e00" ] || fail "the binary is not x86-64; build the Linux x86-64 client"
digest=$(sha256sum "$binary" | cut -d' ' -f1)
jq -n --arg sha "$sha" --arg digest "$digest" --arg binary "$binary" --arg built "$(date -u +%FT%TZ)" \
  '{client_sha:$sha, client_binary_sha256:$digest, binary:$binary, built_at_utc:$built}' >"$out"
echo "built and recorded $sha $digest"
