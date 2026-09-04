#!/bin/sh
set -eu

if [ "$#" -eq 0 ]; then
    printf '%s\n' "usage: horizon-agent-session <command> [argument ...]" >&2
    exit 64
fi

export PATH="/usr/local/cargo/bin:${PATH}"

if [ -f /run/horizon/github-token ]; then
    GITHUB_TOKEN=$(cat /run/horizon/github-token)
    export GITHUB_TOKEN
    GH_TOKEN=${GITHUB_TOKEN}
    export GH_TOKEN
fi

export HORIZON=1
exec "$@"
