#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
source "$repo_root/scripts/version-lib.sh"

version="${1:-}"

if [[ -z "$version" ]]; then
  echo "Usage: ./scripts/set-release-version.sh <version>" >&2
  exit 1
fi

if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-(alpha|beta)\.[0-9]+)?$ ]]; then
  echo "Unsupported release version: $version" >&2
  exit 1
fi

rewrite_workspace_versions "$version"
(cd "$repo_root" && cargo generate-lockfile)
