#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
source "$repo_root/scripts/version-lib.sh"

workspace_version="$(workspace_base_version)"
horizon_core_version="$(workspace_horizon_core_version)"

if [[ -z "$workspace_version" || -z "$horizon_core_version" ]]; then
  echo "Failed to parse version values from Cargo.toml." >&2
  exit 1
fi

if [[ "$workspace_version" != "$horizon_core_version" ]]; then
  cat <<EOF >&2
Version mismatch:
  Cargo.toml [workspace.package].version:           $workspace_version
  Cargo.toml [workspace.dependencies].horizon-core: $horizon_core_version
Update Cargo.toml so the workspace package version and horizon-core workspace dependency version match.
EOF
  exit 1
fi

echo "Version sync check passed: $workspace_version"
