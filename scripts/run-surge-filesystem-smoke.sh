#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Run the local Surge filesystem installer/update smoke for the current OS.

Usage:
  run-surge-filesystem-smoke.sh \
    [--rid <linux-x64|osx-arm64|osx-x64|win-x64>] \
    [--version-a <version>] \
    [--version-b <version>] \
    [--profile <debug|release>] \
    [--toolchain-version <git-tag>] \
    [--surge-path <path>] \
    [--surge-repo-url <https-url>] \
    [--surge-commit-sha <git-sha>] \
    [--store-dir <path>] \
    [--manifest-path <path>] \
    [--install-root <path>] \
    [--binary <path>] \
    [--skip-build] \
    [--skip-toolchain-build]
EOF
}

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "${script_dir}/.." && pwd)"

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    printf 'Required command not found: %s\n' "$1" >&2
    exit 1
  fi
}

output_contains() {
  local needle="$1"

  if command -v rg >/dev/null 2>&1; then
    rg -q --fixed-strings "$needle"
  else
    grep -Fq -- "$needle"
  fi
}

current_os() {
  uname -s
}

current_arch() {
  uname -m
}

is_windows_shell() {
  case "$(current_os)" in
    MINGW*|MSYS*|CYGWIN*) return 0 ;;
    *) return 1 ;;
  esac
}

detect_rid() {
  local os
  local arch
  os="$(current_os)"
  arch="$(current_arch)"

  case "$os" in
    Linux)
      if [ "$arch" = "x86_64" ]; then
        printf 'linux-x64\n'
        return 0
      fi
      ;;
    Darwin)
      if [ "$arch" = "arm64" ]; then
        printf 'osx-arm64\n'
        return 0
      fi
      if [ "$arch" = "x86_64" ]; then
        printf 'osx-x64\n'
        return 0
      fi
      ;;
    MINGW*|MSYS*|CYGWIN*)
      if [ "$arch" = "x86_64" ]; then
        printf 'win-x64\n'
        return 0
      fi
      ;;
  esac

  printf 'Unsupported OS/arch combination: %s/%s\n' "$os" "$arch" >&2
  exit 1
}

to_native_path() {
  local path="$1"

  if is_windows_shell && command -v cygpath >/dev/null 2>&1; then
    cygpath -w "$path"
  else
    printf '%s\n' "$path"
  fi
}

to_posix_path() {
  local path="$1"

  if is_windows_shell && command -v cygpath >/dev/null 2>&1; then
    cygpath -u "$path"
  else
    printf '%s\n' "$path"
  fi
}

to_mixed_path() {
  local path="$1"

  if is_windows_shell && command -v cygpath >/dev/null 2>&1; then
    cygpath -m "$path"
  else
    printf '%s\n' "$path"
  fi
}

to_file_git_url() {
  local path="$1"
  local normalized_path

  if is_windows_shell && command -v cygpath >/dev/null 2>&1; then
    normalized_path="$(cygpath -m "$path")"
    printf 'file:///%s\n' "${normalized_path// /%20}"
  else
    printf 'file://%s\n' "${path// /%20}"
  fi
}

path_exists() {
  [ -e "$1" ]
}

file_contains_literal() {
  local path="$1"
  local needle="$2"

  if command -v rg >/dev/null 2>&1; then
    rg -q --fixed-strings "$needle" "$path"
  else
    grep -Fq -- "$needle" "$path"
  fi
}

configure_windows_linker() {
  local candidates
  local candidate
  local linker_path=""
  local normalized_candidate

  if ! is_windows_shell; then
    return 0
  fi

  candidates="$(cmd.exe //d //c "where link.exe" 2>/dev/null | tr -d '\r' || true)"
  while IFS= read -r candidate; do
    [ -n "$candidate" ] || continue
    normalized_candidate="$(printf '%s' "$candidate" | tr '[:upper:]' '[:lower:]' | tr '\\' '/')"
    case "$normalized_candidate" in
      *'git/usr/bin/link.exe')
        continue
        ;;
    esac
    linker_path="$candidate"
    break
  done <<<"$candidates"

  if [ -z "$linker_path" ]; then
    printf 'Failed to locate MSVC link.exe. Run the Windows smoke from a Visual Studio developer shell.\n' >&2
    exit 1
  fi

  if command -v cygpath >/dev/null 2>&1; then
    linker_path="$(cygpath -m "$linker_path")"
  fi

  export CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER="$linker_path"
  export RUSTC_LINKER="$linker_path"
  printf 'Using Windows linker %s\n' "$CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER"
}

run_helper() {
  local expect="$1"
  shift

  local output
  output="$("$@" 2>&1)"
  printf '%s\n' "$output"

  case "$expect" in
    no-update)
      printf '%s\n' "$output" | output_contains "no update available:"
      ;;
    applied)
      printf '%s\n' "$output" | output_contains "update applied:"
      ;;
    launched)
      printf '%s\n' "$output" | output_contains "update available:"
      ;;
    *)
      printf 'Unknown expectation: %s\n' "$expect" >&2
      exit 1
      ;;
  esac
}

launch_installed_app() {
  local app_exe_native="$1"
  local app_exe_posix="$2"

  if is_windows_shell; then
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -Command \
      "\$p = Start-Process -FilePath '$app_exe_native' -PassThru; Start-Sleep -Seconds 5; if (\$p.HasExited) { throw 'installed Horizon exited immediately' }; Stop-Process -Id \$p.Id"
    return 0
  fi

  "$app_exe_posix" >/tmp/horizon-surge-launch.log 2>&1 &
  local launch_pid=$!
  sleep 5
  if ! kill -0 "$launch_pid" >/dev/null 2>&1; then
    printf 'Installed Horizon exited immediately.\n' >&2
    wait "$launch_pid" || true
    return 1
  fi
  kill "$launch_pid" >/dev/null 2>&1 || true
  wait "$launch_pid" || true
}

run_cargo() {
  (
    cd "$repo_root"
    cargo "${cargo_global_args[@]}" "$@"
  )
}

rid=""
version_a="0.2.0-smoke.1"
version_b="0.2.0-smoke.2"
profile="debug"
toolchain_version="v1.0.0-beta.2"
surge_path=""
surge_repo_url=""
surge_commit_sha=""
store_dir=""
manifest_path=""
install_root=""
binary_path=""
skip_build=false
skip_toolchain_build=false

while [ "$#" -gt 0 ]; do
  case "$1" in
    --rid)
      rid="${2:-}"
      shift 2
      ;;
    --version-a)
      version_a="${2:-}"
      shift 2
      ;;
    --version-b)
      version_b="${2:-}"
      shift 2
      ;;
    --profile)
      profile="${2:-}"
      shift 2
      ;;
    --toolchain-version)
      toolchain_version="${2:-}"
      shift 2
      ;;
    --surge-path)
      surge_path="${2:-}"
      shift 2
      ;;
    --surge-repo-url)
      surge_repo_url="${2:-}"
      shift 2
      ;;
    --surge-commit-sha)
      surge_commit_sha="${2:-}"
      shift 2
      ;;
    --store-dir)
      store_dir="${2:-}"
      shift 2
      ;;
    --manifest-path)
      manifest_path="${2:-}"
      shift 2
      ;;
    --install-root)
      install_root="${2:-}"
      shift 2
      ;;
    --binary)
      binary_path="${2:-}"
      shift 2
      ;;
    --skip-build)
      skip_build=true
      shift
      ;;
    --skip-toolchain-build)
      skip_toolchain_build=true
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

require_command cargo
require_command git

if [ -z "$rid" ]; then
  rid="$(detect_rid)"
fi

case "$profile" in
  debug|release) ;;
  *)
    printf 'Unsupported profile: %s\n' "$profile" >&2
    exit 1
    ;;
esac

if [ "$rid" = "win-x64" ]; then
  configure_windows_linker
fi

case "$rid" in
  linux-x64)
    app_id="horizon-linux-x64"
    main_exe="horizon"
    binary_relpath="target/$profile/horizon"
    installer_ext="bin"
    install_root_default="$HOME/.local/share/horizon"
    ;;
  osx-arm64|osx-x64)
    app_id="horizon-$rid"
    main_exe="horizon"
    binary_relpath="target/$profile/horizon"
    installer_ext="bin"
    install_root_default="$HOME/Library/Application Support/horizon"
    ;;
  win-x64)
    app_id="horizon-win-x64"
    main_exe="horizon.exe"
    binary_relpath="target/$profile/horizon.exe"
    installer_ext="exe"
    if [ -z "${LOCALAPPDATA:-}" ]; then
      printf 'LOCALAPPDATA is required for Windows smoke.\n' >&2
      exit 1
    fi
    install_root_default="$(to_posix_path "${LOCALAPPDATA}")/horizon"
    ;;
  *)
    printf 'Unsupported rid: %s\n' "$rid" >&2
    exit 1
    ;;
esac

if [ -z "$store_dir" ]; then
  store_dir="$repo_root/.surge/smoke/$rid/store"
fi

if [ -z "$manifest_path" ]; then
  manifest_path="$repo_root/.surge/smoke/$rid/manifest.yml"
fi

if [ -z "$install_root" ]; then
  install_root="$install_root_default"
fi

if [ -z "$binary_path" ]; then
  binary_path="$repo_root/$binary_relpath"
fi

toolchain_dir="$repo_root/.surge/toolchain-bin"
toolchain_source_root="$repo_root/.surge/toolchain-src"
if [ -n "$surge_path" ]; then
  toolchain_source_root="$(cd -- "$surge_path" && pwd)"
fi
packages_dir="$repo_root/.surge/packages"
installer_path="$repo_root/.surge/installers/$app_id/$rid/Setup-$rid-$app_id-stable-online-gui.$installer_ext"
app_exe_posix="$install_root/app/$main_exe"
app_exe_native="$(to_native_path "$app_exe_posix")"
install_root_native="$(to_native_path "$install_root")"
store_dir_native="$(to_native_path "$store_dir")"
cargo_config_path="$repo_root/.surge/smoke/$rid/cargo-config.toml"
cargo_global_args=()
patch_surge_core=false
surge_patch_rev=""
surge_patch_url=""

mkdir -p "$(dirname -- "$manifest_path")"

if [ "$skip_toolchain_build" = false ]; then
  build_toolchain_args=(--output-dir "$toolchain_dir")
  if [ -n "$surge_path" ]; then
    build_toolchain_args+=(--source-path "$surge_path")
    printf 'Building Surge toolchain from local source %s\n' "$surge_path"
  elif [ -n "$surge_commit_sha" ]; then
    build_toolchain_args+=(--commit-sha "$surge_commit_sha")
    if [ -n "$surge_repo_url" ]; then
      build_toolchain_args+=(--repo-url "$surge_repo_url")
    fi
    printf 'Building Surge toolchain from %s at %s\n' "${surge_repo_url:-https://github.com/fintermobilityas/surge.git}" "$surge_commit_sha"
  else
    build_toolchain_args+=(--version "$toolchain_version")
    if [ -n "$surge_repo_url" ]; then
      build_toolchain_args+=(--repo-url "$surge_repo_url")
    fi
    printf 'Building Surge toolchain %s\n' "$toolchain_version"
  fi
  (cd "$repo_root" && ./scripts/build-surge-toolchain.sh "${build_toolchain_args[@]}")
fi

if [ -n "$surge_path" ] || [ -n "$surge_commit_sha" ]; then
  patch_surge_core=true
fi

if [ "$patch_surge_core" = true ]; then
  if ! git -C "$toolchain_source_root" rev-parse HEAD >/dev/null 2>&1; then
    printf 'Smoke run requires a Git-backed Surge source checkout at %s. Rerun without --skip-toolchain-build.\n' "$toolchain_source_root" >&2
    exit 1
  fi

  surge_patch_rev="$(git -C "$toolchain_source_root" rev-parse HEAD)"
  if [ -n "$surge_commit_sha" ] && [ "$surge_patch_rev" != "$surge_commit_sha" ]; then
    printf 'Prepared Surge source is at %s, expected %s.\n' "$surge_patch_rev" "$surge_commit_sha" >&2
    exit 1
  fi
  surge_patch_url="$(to_file_git_url "$toolchain_source_root")"

  cat >"$cargo_config_path" <<EOF
[patch."https://github.com/fintermobilityas/surge.git"]
surge-core = { git = "$surge_patch_url", rev = "$surge_patch_rev", package = "surge-core" }
EOF
  cargo_global_args=(--config "$cargo_config_path")
fi

if [ "$skip_build" = false ]; then
  printf 'Building Horizon %s binary\n' "$profile"
  if [ "$profile" = "release" ]; then
    run_cargo build --release
  else
    run_cargo build
  fi
fi

export PATH="$toolchain_dir:$PATH"
require_command surge

printf 'Cleaning previous smoke state for %s\n' "$rid"
rm -rf "$store_dir" "$install_root" "$repo_root/.surge/artifacts/$app_id" "$repo_root/.surge/installers/$app_id"
mkdir -p "$store_dir"
if [ -d "$repo_root/.surge/packages" ]; then
  find "$repo_root/.surge/packages" -maxdepth 1 -type f -name "$app_id-*" -delete
fi

cat >"$manifest_path" <<EOF
schema: 1
storage:
  provider: filesystem
  bucket: ${store_dir_native}

apps:
  - id: ${app_id}
    name: Horizon
    main: ${main_exe}
    installDirectory: horizon
    icon: assets/icons/icon-512.png
    channels: [stable, beta]
    shortcuts: [desktop, start_menu]
    installers: [online-gui]
    target:
      rid: ${rid}
EOF

printf 'Staging Horizon binary for %s\n' "$version_a"
(cd "$repo_root" && ./scripts/stage-surge-artifacts.sh --app-id "$app_id" --rid "$rid" --version "$version_a" --binary "$binary_path" --main-exe "$main_exe")

printf 'Packing %s\n' "$version_a"
(cd "$repo_root" && surge --manifest-path "$manifest_path" pack --app-id "$app_id" --rid "$rid" --version "$version_a" --artifacts-dir "$repo_root/.surge/artifacts/$app_id/$rid/$version_a" --output-dir "$packages_dir")

printf 'Publishing %s to beta and promoting to stable\n' "$version_a"
(cd "$repo_root" && surge --manifest-path "$manifest_path" push --app-id "$app_id" --rid "$rid" --version "$version_a" --channel beta --packages-dir "$packages_dir")
(cd "$repo_root" && surge --manifest-path "$manifest_path" promote --app-id "$app_id" --rid "$rid" --version "$version_a" --channel stable)

if [ "$installer_ext" = "bin" ]; then
  chmod +x "$installer_path"
fi

printf 'Running installer %s\n' "$installer_path"
"$installer_path" --headless

if ! path_exists "$install_root/app/.surge/runtime.yml"; then
  printf 'Runtime manifest missing after install: %s\n' "$install_root/app/.surge/runtime.yml" >&2
  exit 1
fi

if ! file_contains_literal "$install_root/app/.surge/runtime.yml" "version: ${version_a}"; then
  printf 'Installed runtime manifest does not contain version %s.\n' "$version_a" >&2
  exit 1
fi

printf 'Launching installed Horizon once\n'
launch_installed_app "$app_exe_native" "$app_exe_posix"

printf 'Verifying no update is visible before promoting %s\n' "$version_b"
run_helper no-update \
  run_cargo run -p horizon-ui --example surge-update-smoke -- --app-exe "$app_exe_native"

printf 'Staging Horizon binary for %s\n' "$version_b"
(cd "$repo_root" && ./scripts/stage-surge-artifacts.sh --app-id "$app_id" --rid "$rid" --version "$version_b" --binary "$binary_path" --main-exe "$main_exe")

printf 'Packing %s\n' "$version_b"
(cd "$repo_root" && surge --manifest-path "$manifest_path" pack --app-id "$app_id" --rid "$rid" --version "$version_b" --artifacts-dir "$repo_root/.surge/artifacts/$app_id/$rid/$version_b" --output-dir "$packages_dir")

printf 'Publishing %s to beta only\n' "$version_b"
(cd "$repo_root" && surge --manifest-path "$manifest_path" push --app-id "$app_id" --rid "$rid" --version "$version_b" --channel beta --packages-dir "$packages_dir")

printf 'Verifying beta-only update stays hidden from stable install\n'
run_helper no-update \
  run_cargo run -p horizon-ui --example surge-update-smoke -- --app-exe "$app_exe_native"

printf 'Promoting %s to stable and applying update\n' "$version_b"
(cd "$repo_root" && surge --manifest-path "$manifest_path" promote --app-id "$app_id" --rid "$rid" --version "$version_b" --channel stable)
run_helper applied \
  run_cargo run -p horizon-ui --example surge-update-smoke -- --apply --app-exe "$app_exe_native"

if ! file_contains_literal "$install_root/app/.surge/runtime.yml" "version: ${version_b}"; then
  printf 'Updated runtime manifest does not contain version %s.\n' "$version_b" >&2
  exit 1
fi

if ! path_exists "$install_root/app-$version_a"; then
  printf 'Expected previous app snapshot missing: %s\n' "$install_root/app-$version_a" >&2
  exit 1
fi

printf 'Relaunching installed Horizon after update\n'
launch_installed_app "$app_exe_native" "$app_exe_posix"

printf 'Smoke completed successfully for %s\n' "$rid"
