#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Render a Surge manifest with an overridden GitHub Releases storage bucket.

Usage:
  render-surge-manifest.sh --input <path> --output <path> --storage-bucket <owner/repo>
EOF
}

input_path=""
output_path=""
storage_bucket=""

while [ "$#" -gt 0 ]; do
  case "$1" in
    --input)
      input_path="${2:-}"
      shift 2
      ;;
    --output)
      output_path="${2:-}"
      shift 2
      ;;
    --storage-bucket)
      storage_bucket="${2:-}"
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

if [ -z "$input_path" ] || [ -z "$output_path" ] || [ -z "$storage_bucket" ]; then
  printf 'The --input, --output, and --storage-bucket arguments are required.\n\n' >&2
  usage >&2
  exit 1
fi

mkdir -p "$(dirname -- "$output_path")"

awk -v storage_bucket="$storage_bucket" '
  BEGIN {
    replaced = 0
  }

  !replaced && /^  bucket:/ {
    print "  bucket: " storage_bucket
    replaced = 1
    next
  }

  {
    print
  }

  END {
    if (!replaced) {
      print "Failed to locate storage bucket in Surge manifest." > "/dev/stderr"
      exit 1
    }
  }
' "$input_path" > "$output_path"
