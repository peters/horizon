#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Stage a Horizon binary and static packaging assets into Surge's expected artifacts directory layout.

Usage:
  stage-surge-artifacts.sh \
    --app-id <app-id> \
    --rid <rid> \
    --version <version> \
    --binary <path> \
    --main-exe <filename>
EOF
}

app_id=""
rid=""
version=""
binary_path=""
main_exe=""

while [ "$#" -gt 0 ]; do
  case "$1" in
    --app-id)
      app_id="${2:-}"
      shift 2
      ;;
    --rid)
      rid="${2:-}"
      shift 2
      ;;
    --version)
      version="${2:-}"
      shift 2
      ;;
    --binary)
      binary_path="${2:-}"
      shift 2
      ;;
    --main-exe)
      main_exe="${2:-}"
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

if [ -z "$app_id" ] || [ -z "$rid" ] || [ -z "$version" ] || [ -z "$binary_path" ] || [ -z "$main_exe" ]; then
  printf 'The --app-id, --rid, --version, --binary, and --main-exe arguments are required.\n\n' >&2
  usage >&2
  exit 1
fi

if [ ! -f "$binary_path" ]; then
  printf 'Binary not found: %s\n' "$binary_path" >&2
  exit 1
fi

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
target_dir="$repo_root/.surge/artifacts/$app_id/$rid/$version"
icon_source="$repo_root/assets/icons/icon-512.png"

if [ ! -f "$icon_source" ]; then
  printf 'Icon not found: %s\n' "$icon_source" >&2
  exit 1
fi

rm -rf "$target_dir"
mkdir -p "$target_dir/assets/icons"

cp "$binary_path" "$target_dir/$main_exe"
cp "$icon_source" "$target_dir/assets/icons/icon-512.png"

if [[ "$main_exe" != *.exe ]]; then
  chmod +x "$target_dir/$main_exe"
fi
