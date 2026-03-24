#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Render WinGet manifests for a Horizon stable release.

Usage:
  render-winget-manifests.sh \
    --output-dir <path> \
    --version <version> \
    --installer-sha <sha256> \
    --release-date <YYYY-MM-DD>
EOF
}

output_dir=""
version=""
installer_sha=""
release_date=""

while [ "$#" -gt 0 ]; do
  case "$1" in
    --output-dir)
      output_dir="${2:-}"
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

if [ -z "$output_dir" ] || [ -z "$version" ] || [ -z "$installer_sha" ] || [ -z "$release_date" ]; then
  printf 'The output dir, version, installer SHA256, and release date are required.\n\n' >&2
  usage >&2
  exit 1
fi

if [[ ! "$release_date" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]]; then
  printf 'Release date must be YYYY-MM-DD: %s\n' "$release_date" >&2
  exit 1
fi

normalized_sha="$(printf '%s' "$installer_sha" | tr '[:lower:]' '[:upper:]')"
if [[ ! "$normalized_sha" =~ ^[A-F0-9]{64}$ ]]; then
  printf 'Installer SHA256 must be a 64-character hexadecimal string: %s\n' "$installer_sha" >&2
  exit 1
fi

mkdir -p "$output_dir"

cat > "$output_dir/Peters.Horizon.yaml" <<EOF
# yaml-language-server: \$schema=https://aka.ms/winget-manifest.version.1.10.0.schema.json

PackageIdentifier: Peters.Horizon
PackageVersion: ${version}
DefaultLocale: en-US
ManifestType: version
ManifestVersion: 1.10.0
EOF

cat > "$output_dir/Peters.Horizon.locale.en-US.yaml" <<EOF
# yaml-language-server: \$schema=https://aka.ms/winget-manifest.defaultLocale.1.10.0.schema.json

PackageIdentifier: Peters.Horizon
PackageVersion: ${version}
PackageLocale: en-US
Publisher: Peter Rekdal Khan-Sunde
PublisherUrl: https://github.com/peters
PublisherSupportUrl: https://github.com/peters/horizon/issues
PackageName: Horizon
PackageUrl: https://github.com/peters/horizon
License: MIT
LicenseUrl: https://github.com/peters/horizon/blob/v${version}/LICENSE
ShortDescription: GPU-accelerated terminal board on an infinite canvas.
Description: |-
  Horizon is a GPU-accelerated terminal board for managing multiple terminal sessions as freely positioned, resizable panels on an infinite canvas.
  It combines workspaces, panel presets, remote hosts, session persistence, and agent-friendly terminal workflows in one desktop app.
Tags:
- terminal
- workspace
- developer-tools
ReleaseNotesUrl: https://github.com/peters/horizon/releases/tag/v${version}
ManifestType: defaultLocale
ManifestVersion: 1.10.0
EOF

cat > "$output_dir/Peters.Horizon.installer.yaml" <<EOF
# yaml-language-server: \$schema=https://aka.ms/winget-manifest.installer.1.10.0.schema.json

PackageIdentifier: Peters.Horizon
PackageVersion: ${version}
InstallerType: portable
Commands:
- horizon
ReleaseDate: ${release_date}
Installers:
- Architecture: x64
  InstallerUrl: https://github.com/peters/horizon/releases/download/v${version}/horizon-windows-x64.exe
  InstallerSha256: ${normalized_sha}
ManifestType: installer
ManifestVersion: 1.10.0
EOF
