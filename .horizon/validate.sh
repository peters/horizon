#!/usr/bin/env bash
set -euo pipefail
mode="${1:-cpu}"
case "$mode" in cpu|gpu) ;; *) printf 'Usage: %s cpu|gpu\n' "$0" >&2; exit 2;; esac
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/workspace/targets/$(basename "$PWD")-$mode}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-8}"
export RUST_TEST_THREADS="${RUST_TEST_THREADS:-1}"
if [ "$mode" = gpu ]; then
    nvidia-smi --query-gpu=name,driver_version,memory.total --format=csv
    nvcc --version
fi
cargo fmt --all -- --check
./scripts/check-maintainability.sh
RUSTFLAGS="-D warnings" cargo test --workspace --locked
RUSTFLAGS="-D warnings" cargo test -p horizon-ui --locked --features speech
cargo clippy --locked --all-targets --features speech,trace-profiling -- -D warnings
cargo clippy --locked --workspace --lib --bins --examples --features speech -- -D warnings -D clippy::unwrap_used -D clippy::expect_used
cargo clippy --locked --workspace --all-targets --features speech -- -D warnings -W clippy::pedantic
if [ "$mode" = gpu ]; then
    cargo build --locked -p horizon-ui --features speech-cuda
fi
