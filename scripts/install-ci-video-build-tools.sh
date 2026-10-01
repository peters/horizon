#!/usr/bin/env bash
set -euo pipefail

# ARM64 uses the platform C compiler's assembler; x86_64 needs NASM.
if [[ "${RUNNER_ARCH:?RUNNER_ARCH is required}" != X64 ]]; then
    exit 0
fi

case "${RUNNER_OS:?RUNNER_OS is required}" in
    Linux)
        bash "$(dirname -- "${BASH_SOURCE[0]}")/install-ci-ubuntu-dependencies.sh" nasm
        nasm -v
        ;;
    macOS)
        if ! command -v nasm >/dev/null 2>&1; then
            brew install nasm
        fi
        nasm -v
        ;;
    Windows)
        pwsh -NoProfile -Command '
            $ErrorActionPreference = "Stop"
            $version = "2.16.03"
            $archive = Join-Path $env:RUNNER_TEMP "horizon-nasm-$version.zip"
            $destination = Join-Path $env:RUNNER_TEMP "horizon-nasm-$version"
            Invoke-WebRequest -Uri "https://www.nasm.us/pub/nasm/releasebuilds/$version/win64/nasm-$version-win64.zip" -OutFile $archive
            $expected = "3ee4782247bcb874378d02f7eab4e294a84d3d15f3f6ee2de2f47a46aa7226e6"
            if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) {
                throw "NASM archive checksum mismatch"
            }
            Expand-Archive -LiteralPath $archive -DestinationPath $destination -Force
            $binaryDirectory = Join-Path $destination "nasm-$version"
            Add-Content -LiteralPath $env:GITHUB_PATH -Value $binaryDirectory -Encoding utf8
            & (Join-Path $binaryDirectory "nasm.exe") -v
            if ($LASTEXITCODE -ne 0) { throw "NASM version check failed" }
        '
        ;;
    *)
        printf 'Unsupported CI runner OS: %s\n' "$RUNNER_OS" >&2
        exit 1
        ;;
esac
