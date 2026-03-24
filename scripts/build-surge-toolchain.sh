#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Build the Surge toolchain binaries needed for Horizon packaging.

Usage:
  build-surge-toolchain.sh --version <git-tag> --output-dir <path>
EOF
}

version=""
output_dir=""

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version)
      version="${2:-}"
      shift 2
      ;;
    --output-dir)
      output_dir="${2:-}"
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

if [ -z "$version" ] || [ -z "$output_dir" ]; then
  printf 'The --version and --output-dir arguments are required.\n\n' >&2
  usage >&2
  exit 1
fi

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
work_root="$repo_root/.surge/toolchain-src"
rm -rf "$work_root"

git clone \
  --depth 1 \
  --branch "$version" \
  --recurse-submodules \
  https://github.com/fintermobilityas/surge.git \
  "$work_root"

(
  cd "$work_root"
  ./scripts/stage-toolchain-artifact.sh --output "$output_dir" --with-gui
)
