#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Package a Horizon release asset from an already-built binary.

Usage:
  package-release-asset.sh --binary <path> --kind <tar|file> --output <path>
EOF
}

binary_path=""
asset_kind=""
output_path=""

while [ "$#" -gt 0 ]; do
  case "$1" in
    --binary)
      binary_path="${2:-}"
      shift 2
      ;;
    --kind)
      asset_kind="${2:-}"
      shift 2
      ;;
    --output)
      output_path="${2:-}"
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

if [ -z "$binary_path" ] || [ -z "$asset_kind" ] || [ -z "$output_path" ]; then
  printf 'The --binary, --kind, and --output arguments are required.\n\n' >&2
  usage >&2
  exit 1
fi

if [ ! -f "$binary_path" ]; then
  printf 'Binary not found: %s\n' "$binary_path" >&2
  exit 1
fi

mkdir -p "$(dirname "$output_path")"

case "$asset_kind" in
  tar)
    chmod +x "$binary_path"
    tar czf "$output_path" \
      -C "$(dirname "$binary_path")" \
      "$(basename "$binary_path")"
    ;;
  file)
    cp "$binary_path" "$output_path"
    ;;
  *)
    printf 'Unsupported asset kind: %s\n' "$asset_kind" >&2
    exit 1
    ;;
esac
