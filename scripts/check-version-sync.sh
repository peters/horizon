#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
source "$repo_root/scripts/version-lib.sh"

workspace_version="$(workspace_base_version)"
horizon_core_version="$(workspace_horizon_core_version)"
horizon_cursor_version="$(workspace_horizon_cursor_version)"

if [[ -z "$workspace_version" || -z "$horizon_core_version" || -z "$horizon_cursor_version" ]]; then
  echo "Failed to parse version values from Cargo.toml." >&2
  exit 1
fi

failed=0

if [[ "$workspace_version" != "$horizon_core_version" ]]; then
  cat <<EOF >&2
Version mismatch:
  Cargo.toml [workspace.package].version:           $workspace_version
  Cargo.toml [workspace.dependencies].horizon-core: $horizon_core_version
Update Cargo.toml so the workspace package version and horizon-core workspace dependency version match.
EOF
  failed=1
fi

if [[ "$workspace_version" != "$horizon_cursor_version" ]]; then
  cat <<EOF >&2
Version mismatch:
  Cargo.toml [workspace.package].version:             $workspace_version
  Cargo.toml [workspace.dependencies].horizon-cursor: $horizon_cursor_version
Update Cargo.toml so the workspace package version and horizon-cursor workspace dependency version match.
EOF
  failed=1
fi

# Check Cargo.lock is in sync
lock_file="$repo_root/Cargo.lock"
if [[ -f "$lock_file" ]]; then
  for crate in horizon-core horizon-cursor horizon-ui; do
    lock_version=$(awk -v pkg="$crate" '
      /^\[\[package\]\]/ { in_pkg = 0 }
      $0 == "name = \"" pkg "\"" { in_pkg = 1; next }
      in_pkg && /^version = / { gsub(/"/, "", $3); print $3; exit }
    ' "$lock_file")
    if [[ -n "$lock_version" && "$lock_version" != "$workspace_version" ]]; then
      echo "Cargo.lock version mismatch: $crate is $lock_version, expected $workspace_version. Run 'cargo check' to regenerate." >&2
      failed=1
    fi
  done
fi

if [[ "$failed" -eq 1 ]]; then
  exit 1
fi

echo "Version sync check passed: $workspace_version"
