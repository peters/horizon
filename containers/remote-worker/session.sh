#!/bin/sh
set -eu

if [ "$#" -eq 0 ]; then
    printf '%s\n' "usage: horizon-agent-session <command> [argument ...]" >&2
    exit 64
fi

export CARGO_HOME="${CARGO_HOME:-/usr/local/cargo}"
export RUSTUP_HOME="${RUSTUP_HOME:-/usr/local/rustup}"
export PATH="${CARGO_HOME}/bin:${PATH}"

# Only the Git credential helper and gh child consume the protected token.
# Ordinary shells, agents and their unrelated children must not inherit it.
unset GH_TOKEN GITHUB_TOKEN GH_ENTERPRISE_TOKEN GITHUB_ENTERPRISE_TOKEN HORIZON_GITHUB_TOKEN

export HORIZON=1
exec "$@"
