#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
source "$repo_root/scripts/version-lib.sh"

usage() {
  cat <<'EOF' >&2
Usage: ./scripts/next-version.sh [--no-fetch] <alpha|beta|stable>
EOF
  exit 1
}

fetch_tags=1

while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-fetch)
      fetch_tags=0
      shift
      ;;
    alpha|beta|stable)
      channel="$1"
      shift
      break
      ;;
    *)
      usage
      ;;
  esac
done

[[ $# -eq 0 ]] || usage
[[ -n "${channel:-}" ]] || usage

base_version="$(workspace_base_version)"
[[ -n "$base_version" ]] || {
  echo "Failed to read [workspace.package].version from Cargo.toml." >&2
  exit 1
}

if [[ "$fetch_tags" -eq 1 ]] && git remote get-url origin >/dev/null 2>&1; then
  git fetch --quiet --tags origin
fi

stable_tag="v${base_version}"

if git rev-parse --verify --quiet "refs/tags/${stable_tag}" >/dev/null; then
  if [[ "$channel" == "stable" ]]; then
    echo "Stable tag ${stable_tag} already exists. Bump Cargo.toml to the next release line first." >&2
    exit 1
  fi

  echo "Stable tag ${stable_tag} already exists. Bump Cargo.toml to the next release line before cutting more prereleases." >&2
  exit 1
fi

if [[ "$channel" == "stable" ]]; then
  echo "$stable_tag"
  exit 0
fi

max_suffix=0
while IFS= read -r tag; do
  suffix="${tag##*.}"
  [[ "$suffix" =~ ^[0-9]+$ ]] || continue
  if (( suffix > max_suffix )); then
    max_suffix="$suffix"
  fi
done < <(git tag --list "v${base_version}-${channel}.*")

echo "v${base_version}-${channel}.$((max_suffix + 1))"
