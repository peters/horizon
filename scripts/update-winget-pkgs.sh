#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Update a checked-out winget-pkgs fork for a Horizon stable release.

Usage:
  update-winget-pkgs.sh \
    --winget-dir <path> \
    --version <version> \
    --installer-sha <sha256> \
    --release-date <YYYY-MM-DD>
EOF
}

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "${script_dir}/.." && pwd)"

winget_dir=""
version=""
installer_sha=""
release_date=""

while [ "$#" -gt 0 ]; do
  case "$1" in
    --winget-dir)
      winget_dir="${2:-}"
      shift 2
      ;;
    --version)
      version="${2:-}"
      shift 2
      ;;
    --installer-sha)
      installer_sha="${2:-}"
      shift 2
      ;;
    --release-date)
      release_date="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      printf 'Unknown argument: %s\n\n' "$1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

if [ -z "$winget_dir" ] || [ -z "$version" ] || [ -z "$installer_sha" ] || [ -z "$release_date" ]; then
  printf 'The winget dir, version, installer SHA256, and release date are required.\n\n' >&2
  usage >&2
  exit 1
fi

if [ ! -d "$winget_dir" ]; then
  printf 'WinGet directory not found: %s\n' "$winget_dir" >&2
  exit 1
fi

manifest_dir="$winget_dir/manifests/p/Peters/Horizon/$version"
mkdir -p "$manifest_dir"

"$repo_root/scripts/render-winget-manifests.sh" \
  --output-dir "$manifest_dir" \
  --version "$version" \
  --installer-sha "$installer_sha" \
  --release-date "$release_date"
