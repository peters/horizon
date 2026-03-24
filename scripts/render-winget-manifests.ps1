[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$OutputDir,

    [Parameter(Mandatory = $true)]
    [string]$Version,

    [Parameter(Mandatory = $true)]
    [string]$InstallerSha256,

    [Parameter(Mandatory = $true)]
    [string]$ReleaseDate
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

if ($ReleaseDate -notmatch '^\d{4}-\d{2}-\d{2}$') {
    throw "Release date must be YYYY-MM-DD: $ReleaseDate"
}

$normalizedSha = $InstallerSha256.ToUpperInvariant()
if ($normalizedSha -notmatch '^[A-F0-9]{64}$') {
    throw "Installer SHA256 must be a 64-character hexadecimal string: $InstallerSha256"
}

[System.IO.Directory]::CreateDirectory($OutputDir) | Out-Null

$utf8NoBom = New-Object System.Text.UTF8Encoding($false)

function Write-Utf8File {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,

        [Parameter(Mandatory = $true)]
        [string]$Content
    )

    $normalizedContent = $Content.TrimStart("`n")
    if (-not $normalizedContent.EndsWith("`n")) {
        $normalizedContent += "`n"
    }

    [System.IO.File]::WriteAllText($Path, $normalizedContent, $utf8NoBom)
}

$versionManifest = @"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.version.1.10.0.schema.json

PackageIdentifier: Peters.Horizon
PackageVersion: $Version
DefaultLocale: en-US
ManifestType: version
ManifestVersion: 1.10.0
"@

$localeManifest = @"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.defaultLocale.1.10.0.schema.json

PackageIdentifier: Peters.Horizon
PackageVersion: $Version
PackageLocale: en-US
Publisher: Peter Rekdal Khan-Sunde
PublisherUrl: https://github.com/peters
PublisherSupportUrl: https://github.com/peters/horizon/issues
PackageName: Horizon
PackageUrl: https://github.com/peters/horizon
License: MIT
LicenseUrl: https://github.com/peters/horizon/blob/v$Version/LICENSE
ShortDescription: GPU-accelerated terminal board on an infinite canvas.
Description: |-
  Horizon is a GPU-accelerated terminal board for managing multiple terminal sessions as freely positioned, resizable panels on an infinite canvas.
  It combines workspaces, panel presets, remote hosts, session persistence, and agent-friendly terminal workflows in one desktop app.
Tags:
- terminal
- workspace
- developer-tools
ReleaseNotesUrl: https://github.com/peters/horizon/releases/tag/v$Version
ManifestType: defaultLocale
ManifestVersion: 1.10.0
"@

$installerManifest = @"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.installer.1.10.0.schema.json

PackageIdentifier: Peters.Horizon
PackageVersion: $Version
InstallerType: portable
Commands:
- horizon
ReleaseDate: $ReleaseDate
Installers:
- Architecture: x64
  InstallerUrl: https://github.com/peters/horizon/releases/download/v$Version/horizon-windows-x64.exe
  InstallerSha256: $normalizedSha
ManifestType: installer
ManifestVersion: 1.10.0
"@

Write-Utf8File -Path (Join-Path $OutputDir "Peters.Horizon.yaml") -Content $versionManifest
Write-Utf8File -Path (Join-Path $OutputDir "Peters.Horizon.locale.en-US.yaml") -Content $localeManifest
Write-Utf8File -Path (Join-Path $OutputDir "Peters.Horizon.installer.yaml") -Content $installerManifest
