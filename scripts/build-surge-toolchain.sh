#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Build the Surge toolchain binaries needed for Horizon packaging.

Usage:
  build-surge-toolchain.sh [--version <git-tag> | --commit-sha <git-sha> | --source-path <path>] [--repo-url <https-url>] --output-dir <path> [--prepare-only]
EOF
}

version=""
commit_sha=""
repo_url="https://github.com/fintermobilityas/surge.git"
source_path=""
output_dir=""
prepare_only=0

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version)
      version="${2:-}"
      shift 2
      ;;
    --commit-sha)
      commit_sha="${2:-}"
      shift 2
      ;;
    --repo-url)
      repo_url="${2:-}"
      shift 2
      ;;
    --source-path)
      source_path="${2:-}"
      shift 2
      ;;
    --output-dir)
      output_dir="${2:-}"
      shift 2
      ;;
    --prepare-only)
      prepare_only=1
      shift
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

if [ -z "$output_dir" ]; then
  printf 'The --output-dir argument is required.\n\n' >&2
  usage >&2
  exit 1
fi

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
work_root="$repo_root/.surge/toolchain-src"
requested_modes=0
[ -n "$version" ] && requested_modes=$((requested_modes + 1))
[ -n "$commit_sha" ] && requested_modes=$((requested_modes + 1))
[ -n "$source_path" ] && requested_modes=$((requested_modes + 1))

if [ "$requested_modes" -eq 0 ]; then
  printf 'Specify one of --version, --commit-sha, or --source-path.\n\n' >&2
  usage >&2
  exit 1
fi

if [ "$requested_modes" -gt 1 ]; then
  printf 'Use only one of --version, --commit-sha, or --source-path.\n\n' >&2
  usage >&2
  exit 1
fi

stamp_matches() {
  local root="$1"
  local expected_ref="$2"
  local expected_commit="$3"

  [ -f "$root/.surge-source-ref" ] || return 1
  [ -f "$root/.surge-source-commit" ] || return 1
  [ "$(cat "$root/.surge-source-ref")" = "$expected_ref" ] || return 1
  [ "$(cat "$root/.surge-source-commit")" = "$expected_commit" ] || return 1
}

write_stamps() {
  local root="$1"
  local source_ref="$2"
  local source_commit="$3"

  mkdir -p "$root"
  printf '%s\n' "$source_ref" >"$root/.surge-source-ref"
  printf '%s\n' "$source_commit" >"$root/.surge-source-commit"
}

toolchain_ready() {
  local root="$1"
  local source_ref="$2"
  local source_commit="$3"
  local runtime_name
  local binaries=()
  local binary

  case "$(uname -s | tr '[:upper:]' '[:lower:]')" in
    linux)
      runtime_name="libsurge.so"
      binaries=(surge surge-supervisor surge-installer surge-installer-ui)
      ;;
    darwin)
      runtime_name="libsurge.dylib"
      binaries=(surge surge-supervisor surge-installer surge-installer-ui)
      ;;
    msys*|mingw*|cygwin*)
      runtime_name="surge.dll"
      binaries=(surge.exe surge-supervisor.exe surge-installer.exe surge-installer-ui.exe)
      ;;
    *)
      printf 'Unsupported host OS.\n' >&2
      exit 1
      ;;
  esac

  stamp_matches "$root" "$source_ref" "$source_commit" || return 1
  [ -f "$root/$runtime_name" ] || return 1
  for binary in "${binaries[@]}"; do
    [ -f "$root/$binary" ] || return 1
  done
}

resolve_tag_commit() {
  local url="$1"
  local tag="$2"
  local tag_sha

  tag_sha="$(git ls-remote --quiet "$url" "refs/tags/${tag}^{}" "refs/tags/${tag}" | awk 'NR==1 { print $1 }')"
  if [ -z "$tag_sha" ]; then
    printf 'Failed to resolve tag %s from %s.\n' "$tag" "$url" >&2
    exit 1
  fi
  printf '%s\n' "$tag_sha"
}

if [ -n "$source_path" ]; then
  source_root="$(cd -- "$source_path" && pwd)"
  if [ ! -f "$source_root/Cargo.toml" ] || [ ! -x "$source_root/scripts/stage-toolchain-artifact.sh" ]; then
    printf 'Invalid Surge source path: %s\n' "$source_root" >&2
    exit 1
  fi
  source_ref="path:${source_root}"
  source_commit="$(git -C "$source_root" rev-parse HEAD)"
else
  if [ -n "$version" ]; then
    source_ref="tag:${version}"
    source_commit="$(resolve_tag_commit "$repo_url" "$version")"
  else
    source_ref="commit:${commit_sha}"
    source_commit="$commit_sha"
  fi

  if stamp_matches "$work_root" "$source_ref" "$source_commit"; then
    printf 'Reusing cached Surge source %s at %s\n' "$source_ref" "$source_commit"
  else
    rm -rf "$work_root"

    if [ -n "$version" ]; then
      git clone \
        --depth 1 \
        --branch "$version" \
        --recurse-submodules \
        "$repo_url" \
        "$work_root"
    else
      git clone \
        --recurse-submodules \
        "$repo_url" \
        "$work_root"
      (
        cd "$work_root"
        git checkout --force "$commit_sha"
        git submodule update --init --recursive
      )
    fi

    actual_commit="$(git -C "$work_root" rev-parse HEAD)"
    if [ "$actual_commit" != "$source_commit" ]; then
      printf 'Prepared Surge source at %s but expected %s.\n' "$actual_commit" "$source_commit" >&2
      exit 1
    fi

    write_stamps "$work_root" "$source_ref" "$source_commit"
  fi

  source_root="$work_root"
fi

if [ "$prepare_only" -eq 1 ]; then
  printf 'Prepared Surge source %s at %s\n' "$source_ref" "$source_commit"
  exit 0
fi

if toolchain_ready "$output_dir" "$source_ref" "$source_commit"; then
  printf 'Reusing cached Surge toolchain %s at %s\n' "$source_ref" "$source_commit"
  exit 0
fi

(
  cd "$source_root"
  ./scripts/stage-toolchain-artifact.sh --output "$output_dir" --with-gui
)
write_stamps "$output_dir" "$source_ref" "$source_commit"
