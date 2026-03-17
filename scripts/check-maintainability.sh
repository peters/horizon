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

run_self_tests() {
  local tmpdir
  tmpdir="$(mktemp -d)"
  trap 'rm -rf "$tmpdir"' RETURN

  cat >"$tmpdir/inline-tests-at-end.rs" <<'EOF'
fn alpha() {}

#[cfg(test)]
mod tests {
    #[test]
    fn works() {}
}
EOF

  cat >"$tmpdir/test_helpers_before_prod.rs" <<'EOF'
#[cfg(test)]
fn helper() {}

fn alpha() {}

#[cfg(test)]
mod tests {
    #[test]
    fn works() {}
}
EOF

  local measured
  measured="$(measure_production_lines "$tmpdir/inline-tests-at-end.rs")"
  if [[ "$measured" != "3" ]]; then
    printf 'maintainability self-test failed: expected 3 lines before inline tests, got %s\n' "$measured" >&2
    status=1
  fi

  measured="$(measure_production_lines "$tmpdir/test_helpers_before_prod.rs")"
  if [[ "$measured" != "6" ]]; then
    printf 'maintainability self-test failed: expected 6 lines including helper spacing before inline tests, got %s\n' \
      "$measured" >&2
    status=1
  fi
}

measure_production_lines() {
  local file="$1"
  awk '
    function flush_pending_attr() {
      if (pending_test_attr) {
        count += 1
        pending_test_attr = 0
      }
    }

    /^[[:space:]]*#\[cfg\(test\)\][[:space:]]*$/ {
      flush_pending_attr()
      pending_test_attr = 1
      next
    }

    pending_test_attr && /^[[:space:]]*(\/\/.*)?$/ {
      next
    }

    pending_test_attr && /^[[:space:]]*mod[[:space:]]+tests[[:space:]]*\{/ {
      exit
    }

    {
      flush_pending_attr()
      count += 1
    }

    END {
      flush_pending_attr()
      print count + 0
    }
  ' "$file"
}

check_file_length() {
  local file="$1"
  local lines
  lines="$(measure_production_lines "$file")"
  if (( lines > MAX_LINES )); then
    printf 'maintainability error: %s has %d lines (limit: %d)\n' \
      "${file#"$ROOT_DIR"/}" "$lines" "$MAX_LINES" >&2
    status=1
  fi
}

run_self_tests

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
