#!/usr/bin/env bash
set -euo pipefail

DURATION="${1:-10}"
shift || true

BINARY="target/profiling/horizon"
OUT_DIR="profiling-output"
TIMESTAMP="$(date +%Y%m%d-%H%M%S)"
REPORT_BASE="$OUT_DIR/nsys-$TIMESTAMP"
REPORT_FILE="$REPORT_BASE.nsys-rep"
SQLITE_FILE="$REPORT_BASE.sqlite"
SUMMARY_FILE="$OUT_DIR/nsys-summary-$TIMESTAMP.txt"
STDERR_FILE="$OUT_DIR/nsys-stderr-$TIMESTAMP.log"
TRACE_APIS="${HORIZON_NSYS_TRACE_APIS:-vulkan,osrt}"
VULKAN_GPU_WORKLOAD="${HORIZON_NSYS_VULKAN_GPU_WORKLOAD:-individual}"
TIMEOUT_SECONDS="${HORIZON_NSYS_TIMEOUT:-$((DURATION + 5))}"

if [ "$#" -eq 0 ]; then
    APP_ARGS=(--blank)
else
    APP_ARGS=("$@")
fi

if ! command -v nsys >/dev/null 2>&1; then
    echo "nsys is not installed; use scripts/profile.sh instead." >&2
    exit 1
fi

mkdir -p "$OUT_DIR"

echo "Building profiling binary..."
cargo build --profile profiling

echo "Launching Horizon under Nsight Systems for ${DURATION}s..."
echo "Trace APIs:  ${TRACE_APIS}"
echo "App args:    ${APP_ARGS[*]}"
echo "Interaction: pan and resize the main window while capture is active"
echo ""

timeout "$TIMEOUT_SECONDS" \
    nsys profile \
    --duration="$DURATION" \
    --force-overwrite=true \
    --export=sqlite \
    --trace="$TRACE_APIS" \
    --vulkan-gpu-workload="$VULKAN_GPU_WORKLOAD" \
    --output="$REPORT_BASE" \
    "$BINARY" "${APP_ARGS[@]}" \
    2>"$STDERR_FILE" || true

if [ ! -f "$REPORT_FILE" ]; then
    echo "Nsight Systems did not produce a report." >&2
    echo "See stderr: $STDERR_FILE" >&2
    exit 1
fi

STATS_INPUT="$REPORT_FILE"
if [ -f "$SQLITE_FILE" ]; then
    STATS_INPUT="$SQLITE_FILE"
fi

nsys stats \
    --report osrt_sum,vulkan_api_sum,vulkan_marker_sum,vulkan_gpu_marker_sum \
    --format column \
    "$STATS_INPUT" >"$SUMMARY_FILE" 2>/dev/null || true

echo ""
echo "Report:   $REPORT_FILE"
if [ -f "$SQLITE_FILE" ]; then
    echo "SQLite:   $SQLITE_FILE"
fi
echo "Summary:  $SUMMARY_FILE"
echo "Stderr:   $STDERR_FILE"
