#!/usr/bin/env bash
set -euo pipefail
CAST_BENCH_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
exec "${CAST_BENCH_PYTHON:-python3}" "$CAST_BENCH_DIR/bench.py" "$@"
