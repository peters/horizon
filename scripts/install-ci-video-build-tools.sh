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
    *)
        printf 'Unsupported CI runner OS: %s\n' "$RUNNER_OS" >&2
        exit 1
        ;;
esac
