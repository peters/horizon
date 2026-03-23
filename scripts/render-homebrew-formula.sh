#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Render the Horizon Homebrew formula for a stable release.

Usage:
  render-homebrew-formula.sh \
    --version <version> \
    --macos-arm64-sha <sha256> \
    --macos-x64-sha <sha256> \
    --linux-x64-sha <sha256>
EOF
}

version=""
macos_arm64_sha=""
macos_x64_sha=""
linux_x64_sha=""

while [ "$#" -gt 0 ]; do
  case "$1" in
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

if [ -z "$version" ] || [ -z "$macos_arm64_sha" ] || [ -z "$macos_x64_sha" ] || [ -z "$linux_x64_sha" ]; then
  printf 'The version and all SHA256 arguments are required.\n\n' >&2
  usage >&2
  exit 1
fi

cat <<EOF
class Horizon < Formula
  desc "GPU-accelerated terminal board on an infinite canvas"
  homepage "https://github.com/peters/horizon"
  url "https://github.com/peters/horizon/releases/download/v${version}/horizon-osx-arm64.tar.gz"
  version "${version}"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/peters/horizon/releases/download/v${version}/horizon-osx-arm64.tar.gz"
      sha256 "${macos_arm64_sha}"
    end

    on_intel do
      url "https://github.com/peters/horizon/releases/download/v${version}/horizon-osx-x64.tar.gz"
      sha256 "${macos_x64_sha}"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/peters/horizon/releases/download/v${version}/horizon-linux-x64.tar.gz"
      sha256 "${linux_x64_sha}"
    end
  end

  def install
    bin.install "horizon"
  end

  test do
    assert_path_exists bin/"horizon"
    assert_predicate bin/"horizon", :executable?
  end
end
EOF
