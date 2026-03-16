#!/usr/bin/env bash
set -euo pipefail

DURATION="${1:-10}"
BINARY="target/profiling/horizon"
OUT_DIR="profiling-output"

mkdir -p "$OUT_DIR"
TIMESTAMP=$(date +%Y%m%d-%H%M%S)
PERF_DATA="$OUT_DIR/perf-$TIMESTAMP.data"
FLAMEGRAPH="$OUT_DIR/flamegraph-$TIMESTAMP.svg"
TOP_FUNCS="$OUT_DIR/top-functions-$TIMESTAMP.txt"
TRACE_LOG="$OUT_DIR/trace-$TIMESTAMP.log"
TOP_SPANS="$OUT_DIR/top-spans-$TIMESTAMP.txt"
PYTHON_BIN="$(command -v python3 || command -v python)"

can_use_perf() {
    local probe
    probe="$(mktemp "$OUT_DIR/perf-probe.XXXXXX.data")"
    if perf record -o "$probe" -- /usr/bin/true >/dev/null 2>&1; then
        rm -f "$probe"
        return 0
    fi
    rm -f "$probe"
    return 1
}

summarize_trace_spans() {
    "$PYTHON_BIN" - "$TRACE_LOG" <<'PY' > "$TOP_SPANS"
import re
import sys
from collections import defaultdict

path = sys.argv[1]
unit_scale = {"ns": 1e-3, "µs": 1.0, "us": 1.0, "ms": 1000.0, "s": 1_000_000.0}

def collect(only_redraw):
    agg = defaultdict(lambda: [0.0, 0])
    with open(path, "r", encoding="utf-8") as fh:
        for line in fh:
            if " close time.busy=" not in line:
                continue
            if only_redraw and "WindowEvent::RedrawRequested" not in line:
                continue
            prefix, rest = line.split(" close time.busy=", 1)
            try:
                left, target_part = prefix.rsplit(" ", 1)
            except ValueError:
                continue
            target = target_part[:-1] if target_part.endswith(":") else target_part
            ancestry = left.split(" INFO ", 1)[1].rstrip(":")
            direct = ancestry.rsplit(":", 1)[-1].strip()
            busy_token = rest.split()[0]
            match = re.fullmatch(r"([0-9.]+)(ns|µs|us|ms|s)", busy_token)
            if not match:
                continue
            busy_us = float(match.group(1)) * unit_scale[match.group(2)]
            agg[(target, direct)][0] += busy_us
            agg[(target, direct)][1] += 1
    return agg

aggregate = collect(only_redraw=True)
if not aggregate:
    aggregate = collect(only_redraw=False)

items = sorted(aggregate.items(), key=lambda item: item[1][0], reverse=True)
print("=== Top traced spans ===")
for (target, direct), (total_us, calls) in items[:40]:
    avg_us = total_us / calls if calls else 0.0
    print(f"{total_us / 1000:9.3f} ms total  {calls:5d} calls  {avg_us:9.3f} us avg  {target}::{direct}")
PY
}

echo "Launching Horizon with perf recording for ${DURATION}s..."
echo ">>> Pan around, interact with terminals, then wait idle <<<"
echo ""

if can_use_perf; then
    echo "Building profiling binary..."
    cargo build --profile profiling

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
else
    echo "perf is unavailable on this machine; falling back to tracing spans."
    echo "Building trace-instrumented profiling binary..."
    cargo build --profile profiling --features trace-profiling

    HORIZON_TRACE_SPANS=1 RUST_LOG=info timeout "$DURATION" "$BINARY" > "$TRACE_LOG" 2>/dev/null || true
    summarize_trace_spans

    echo ""
    cat "$TOP_SPANS"
    echo ""
    echo "Trace log:  $TRACE_LOG"
fi
