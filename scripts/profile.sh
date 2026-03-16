#!/usr/bin/env bash
set -euo pipefail

DURATION="${1:-10}"
BINARY="target/profiling/horizon"
OUT_DIR="profiling-output"

if [ ! -f "$BINARY" ]; then
    echo "Building profiling binary..."
    cargo build --profile profiling
fi

mkdir -p "$OUT_DIR"
TIMESTAMP=$(date +%Y%m%d-%H%M%S)
PERF_DATA="$OUT_DIR/perf-$TIMESTAMP.data"
FLAMEGRAPH="$OUT_DIR/flamegraph-$TIMESTAMP.svg"
TOP_FUNCS="$OUT_DIR/top-functions-$TIMESTAMP.txt"

echo "Launching Horizon with perf recording for ${DURATION}s..."
echo ">>> Pan around, interact with terminals, then wait idle <<<"
echo ""

perf record -g --call-graph dwarf -F 99 -o "$PERF_DATA" -- timeout "$DURATION" "$BINARY" 2>/dev/null || true

echo ""
echo "Generating flamegraph..."
perf script -i "$PERF_DATA" 2>/dev/null | flamegraph --minwidth 0.5 > "$FLAMEGRAPH" 2>/dev/null

echo "Extracting top functions..."
perf report -i "$PERF_DATA" --stdio --no-children -g none --percent-limit 1.0 2>/dev/null | head -60 > "$TOP_FUNCS"

echo ""
echo "=== Top CPU consumers ==="
cat "$TOP_FUNCS"
echo ""
echo "Flamegraph: $FLAMEGRAPH"
echo "Raw data:   $PERF_DATA"
