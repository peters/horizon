#!/usr/bin/env bash
# Run the worker image's Python suite one module per process, in parallel.
#
# The suite drives the worker and the browser MCP as real processes, so it spends
# most of its time waiting: 609 tests took 162 s in one process and 58 s across
# four. Each module gets its own interpreter, which also keeps one module's
# patched module state and file locks out of another's.
#
# Usage: run-cloud-worker-tests.sh <LOG> [JOBS] [--module-dir DIR]
#
# Writes the combined output to LOG and each module's own log beside it in
# `<LOG>.d/`. Exits 0 when every module passed, 1 when a module failed, and 3
# when the run skipped a test, which the caller decides how to treat: a skip
# usually means a prerequisite is missing.
set -euo pipefail

if [ "$#" -lt 1 ]; then
  echo "usage: $(basename "$0") <LOG> [JOBS] [--module-dir DIR]" >&2
  exit 2
fi

log=$1
jobs=4
module_dir=examples/cloud-worker
shift
while [ "$#" -gt 0 ]; do
  case "$1" in
    --module-dir)
      module_dir=${2:?--module-dir needs a folder}
      shift 2
      ;;
    *)
      jobs=$1
      shift
      ;;
  esac
done

mkdir -p "$(dirname "$log")"
per_module="${log%.log}.d"
rm -rf "$per_module"
mkdir -p "$per_module"

# A module is a `test_*.py` file at the top of the folder, which is what
# `unittest discover` reads there.
modules=$(cd "$module_dir" && ls test_*.py | sed 's/\.py$//')
if [ -z "$modules" ]; then
  echo "no test modules in $module_dir" >&2
  exit 2
fi

export HORIZON_CLOUD_WORKER_MODULE_DIR=$module_dir
export HORIZON_CLOUD_WORKER_LOG_DIR=$per_module

# `xargs` returns 123 when any child failed, and the per-module logs say which.
status=0
printf '%s\n' "$modules" \
  | xargs -P "$jobs" -I{} bash -c '
      cd "$HORIZON_CLOUD_WORKER_MODULE_DIR" || exit 1
      python3 -B -m unittest {} > "$HORIZON_CLOUD_WORKER_LOG_DIR/{}.log" 2>&1
    ' || status=$?

cat "$per_module"/*.log > "$log"

# `awk` sums the per-module figures, so the run needs no tool beyond the shell.
# A pattern no module printed makes `grep` exit 1, which is not a failure here.
sum_of() {
  grep -hoE "$1" "$per_module"/*.log | grep -oE "[0-9]+" | awk '{sum += $1} END {print sum + 0}' || true
}
ran=$(sum_of "^Ran [0-9]+")
skipped=$(sum_of "skipped=[0-9]+")
echo "Ran ${ran:-0} tests across $(printf '%s\n' "$modules" | wc -l) modules with $jobs jobs"

if [ "$status" -ne 0 ]; then
  echo "A module failed:" >&2
  grep -lE "^(FAIL|ERROR):" "$per_module"/*.log | sed 's/^/  /' >&2
  exit 1
fi
if [ "${skipped:-0}" -gt 0 ]; then
  exit 3
fi
