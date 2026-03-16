#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MAX_LINES=1000
ALLOW_PATTERN='#\[allow\(clippy::too_many_lines\)\]'
SOURCE_DIRS=(
  "$ROOT_DIR/crates/horizon-core/src"
  "$ROOT_DIR/crates/horizon-ui/src"
)
status=0

check_file_length() {
  local file="$1"
  local lines
  lines=$(
    awk '
      /^[[:space:]]*#\[cfg\(test\)\]/ { exit }
      { count += 1 }
      END { print count + 0 }
    ' "$file"
  )
  if (( lines > MAX_LINES )); then
    printf 'maintainability error: %s has %d lines (limit: %d)\n' \
      "${file#"$ROOT_DIR"/}" "$lines" "$MAX_LINES" >&2
    status=1
  fi
}

while IFS= read -r -d '' file; do
  check_file_length "$file"
done < <(
  find \
    "${SOURCE_DIRS[@]}" \
    -type f \
    -name '*.rs' \
    -print0
)

if command -v rg >/dev/null 2>&1; then
  if rg -n "$ALLOW_PATTERN" "${SOURCE_DIRS[@]}" >/dev/null; then
    echo 'maintainability error: remove #[allow(clippy::too_many_lines)] from core/UI source files' >&2
    rg -n "$ALLOW_PATTERN" "${SOURCE_DIRS[@]}" >&2
    status=1
  fi
elif grep -R -n -E "$ALLOW_PATTERN" "${SOURCE_DIRS[@]}" >/dev/null; then
  echo 'maintainability error: remove #[allow(clippy::too_many_lines)] from core/UI source files' >&2
  grep -R -n -E "$ALLOW_PATTERN" "${SOURCE_DIRS[@]}" >&2
  status=1
fi

exit "$status"
