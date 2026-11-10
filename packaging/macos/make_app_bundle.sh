#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
LOCAL_APP_BUNDLE="$ROOT_DIR/target/Horizon.app"
BINARY="$ROOT_DIR/target/release/horizon"

usage() {
  cat <<EOF
Usage: $(basename "$0") [OPTIONS]

Assembles a Horizon.app bundle under target/Horizon.app.

Options:
  --install                 Copy the built bundle into the applications directory.
  --applications-dir <dir>  Destination for --install (default: \$APPLICATIONS_DIR or /Applications).
  --version <version>       Set version of Horizon build.
  -h                        Show this help message and exit.
EOF
}

INSTALL=0
APPLICATIONS_DIR="${APPLICATIONS_DIR:-/Applications}"
VERSION=""

resolve_path() {
  local path="$1"
  local parent_dir
  local base_name

  if [[ -e "$path" ]]; then
    (cd -- "$path" && pwd -P)
    return
  fi

  parent_dir="$(dirname -- "$path")"
  base_name="$(basename -- "$path")"
  install -d "$parent_dir"
  printf '%s/%s\n' "$(cd -- "$parent_dir" && pwd -P)" "$base_name"
}

read_binary_minimum_macos_version() {
  otool -l "$BINARY" | awk '
    $1 == "minos" { print $2; exit }
    saw_version_min && $1 == "version" { print $2; exit }
    $1 == "cmd" && $2 == "LC_VERSION_MIN_MACOSX" { saw_version_min = 1; next }
    $1 == "cmd" { saw_version_min = 0 }
  '
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --install) INSTALL=1; shift ;;
    --applications-dir)
      [[ $# -gt 1 ]] || { printf 'Error: --applications-dir requires an argument\n' >&2; exit 1; }
      APPLICATIONS_DIR="$2"; shift 2 ;;
    --version)
      [[ $# -gt 1 ]] || { printf 'Error: --version requires an argument\n' >&2; exit 1; }
      VERSION="$2"; shift 2 ;;
    -h) usage; exit 0 ;;
    *) printf 'Unknown option: %s\n' "$1" >&2; usage >&2; exit 1 ;;
  esac
done

if [[ ! -x "$BINARY" ]]; then
  printf 'Error: binary not found at %s\n' "$BINARY" >&2
  printf 'Run `cargo build --release` first.\n' >&2
  exit 1
fi

MINIMUM_MACOS_VERSION="$(read_binary_minimum_macos_version)"
if [[ -z "$MINIMUM_MACOS_VERSION" ]]; then
  printf 'Error: failed to determine minimum macOS version from %s\n' "$BINARY" >&2
  exit 1
fi

# Convert the shipped 512px app icon directly into an .icns file.
ICON_TMP_DIR="$(mktemp -d)"
ICNS="$ICON_TMP_DIR/Horizon.icns"
trap 'rm -rf "$ICON_TMP_DIR"' EXIT
sips -s format icns "$ROOT_DIR/assets/icons/icon-512.png" --out "$ICNS" >/dev/null

# Assemble the .app bundle locally under target/
rm -rf "$LOCAL_APP_BUNDLE"
install -d "$LOCAL_APP_BUNDLE/Contents/MacOS"
install -d "$LOCAL_APP_BUNDLE/Contents/Resources"

install -m 0755 "$BINARY" "$LOCAL_APP_BUNDLE/Contents/MacOS/horizon"
install -m 0644 "$ICNS" "$LOCAL_APP_BUNDLE/Contents/Resources/Horizon.icns"

# Build version fields conditionally
VERSION_FIELDS=""
if [[ -n "$VERSION" ]]; then
  VERSION_FIELDS="  <key>CFBundleVersion</key>
  <string>${VERSION}</string>
  <key>CFBundleShortVersionString</key>
  <string>${VERSION}</string>
"
fi

cat > "$LOCAL_APP_BUNDLE/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>
  <string>Horizon</string>
  <key>CFBundleDisplayName</key>
  <string>Horizon</string>
  <key>CFBundleIdentifier</key>
  <string>com.github.peters.horizon</string>
${VERSION_FIELDS}  <key>CFBundleExecutable</key>
  <string>horizon</string>
  <key>CFBundleIconFile</key>
  <string>Horizon</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>LSMinimumSystemVersion</key>
  <string>${MINIMUM_MACOS_VERSION}</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key>
  <true/>
</dict>
</plist>
PLIST

printf 'Built Horizon.app at %s\n' "$LOCAL_APP_BUNDLE"

if [[ "$INSTALL" -eq 1 ]]; then
  install -d "$APPLICATIONS_DIR"
  DESTINATION_APP_BUNDLE="$APPLICATIONS_DIR/Horizon.app"
  SOURCE_APP_BUNDLE_PATH="$(resolve_path "$LOCAL_APP_BUNDLE")"
  DESTINATION_APP_BUNDLE_PATH="$(resolve_path "$DESTINATION_APP_BUNDLE")"
  if [[ "$SOURCE_APP_BUNDLE_PATH" == "$DESTINATION_APP_BUNDLE_PATH" ]]; then
    printf 'Skipped install because the bundle is already at %s\n' "$SOURCE_APP_BUNDLE_PATH"
    exit 0
  fi

  rm -rf "$DESTINATION_APP_BUNDLE"
  cp -R "$LOCAL_APP_BUNDLE" "$DESTINATION_APP_BUNDLE"
  printf 'Installed Horizon.app to %s\n' "$APPLICATIONS_DIR"
fi
