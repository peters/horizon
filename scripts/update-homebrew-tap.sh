#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Update a checked-out Homebrew tap repo for a Horizon stable release.

Usage:
  update-homebrew-tap.sh \
    --tap-dir <path> \
    --version <version> \
    --macos-arm64-sha <sha256> \
    --macos-x64-sha <sha256> \
    --linux-x64-sha <sha256>
EOF
}

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "${script_dir}/.." && pwd)"

tap_dir=""
version=""
macos_arm64_sha=""
macos_x64_sha=""
linux_x64_sha=""

while [ "$#" -gt 0 ]; do
  case "$1" in
    --tap-dir)
      tap_dir="${2:-}"
      shift 2
      ;;
    --version)
      version="${2:-}"
      shift 2
      ;;
    --macos-arm64-sha)
      macos_arm64_sha="${2:-}"
      shift 2
      ;;
    --macos-x64-sha)
      macos_x64_sha="${2:-}"
      shift 2
      ;;
    --linux-x64-sha)
      linux_x64_sha="${2:-}"
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

if [ -z "$tap_dir" ] || [ -z "$version" ] || [ -z "$macos_arm64_sha" ] || [ -z "$macos_x64_sha" ] || [ -z "$linux_x64_sha" ]; then
  printf 'The tap dir, version, and all SHA256 arguments are required.\n\n' >&2
  usage >&2
  exit 1
fi

if [ ! -d "$tap_dir" ]; then
  printf 'Tap directory not found: %s\n' "$tap_dir" >&2
  exit 1
fi

mkdir -p "$tap_dir/Formula"

"$repo_root/scripts/render-homebrew-formula.sh" \
  --version "$version" \
  --macos-arm64-sha "$macos_arm64_sha" \
  --macos-x64-sha "$macos_x64_sha" \
  --linux-x64-sha "$linux_x64_sha" \
  > "$tap_dir/Formula/horizon.rb"

cat > "$tap_dir/README.md" <<EOF
# homebrew-horizon

Homebrew tap for [Horizon](https://github.com/peters/horizon), the GPU-accelerated terminal board on an infinite canvas.

## Install

\`\`\`bash
brew tap peters/horizon
brew install horizon
\`\`\`

Or install in one command:

\`\`\`bash
brew install peters/horizon/horizon
\`\`\`

## Upgrade

\`\`\`bash
brew upgrade horizon
\`\`\`

## Uninstall

\`\`\`bash
brew uninstall horizon
brew untap peters/horizon
\`\`\`

## Release Source

The formula installs the stable release assets published at:

https://github.com/peters/horizon/releases/tag/v${version}
EOF
