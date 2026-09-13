#!/usr/bin/env bash
# Packaging-readiness invariants for Horizon browser crates.
# Does not publish anything.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

status=0

package_publish() {
  local crate="$1"
  awk -v crate="$crate" '
    $0 == "[package]" { in_package = 1; next }
    in_package && /^\[/ { in_package = 0 }
    in_package && $1 == "name" {
      gsub(/"/, "", $3)
      name = $3
    }
    in_package && $1 == "publish" {
      if (name == crate) {
        print $0
        exit
      }
    }
  ' "crates/${crate}/Cargo.toml"
}

expect_publish() {
  local crate="$1"
  local expected="$2"
  local actual
  actual="$(package_publish "$crate")"
  if [[ "$actual" != "$expected" ]]; then
    printf 'packaging: %s publish is %q, expected %q\n' "$crate" "$actual" "$expected" >&2
    status=1
  fi
}

expect_publish horizon-browser 'publish = ["crates-io"]'
expect_publish horizon-browser-protocol 'publish = false'
expect_publish horizon-browser-mcp 'publish = false'
expect_publish horizon-browser-cli 'publish = false'

forbidden='horizon-ui|horizon-core|horizon-browser-mcp|horizon-browser-cli|browser-smoke|\.github/'

if cargo package -p horizon-browser-protocol --locked --allow-dirty --list | grep -E "$forbidden" >/dev/null; then
  printf 'packaging: horizon-browser-protocol package contains product or smoke paths\n' >&2
  status=1
fi

if cargo package -p horizon-browser --locked --allow-dirty --list --no-verify | grep -E "$forbidden" >/dev/null; then
  printf 'packaging: horizon-browser package contains product or smoke paths\n' >&2
  status=1
fi

if cargo package -p horizon-browser --locked --allow-dirty >/tmp/horizon-browser-package-verify.log 2>&1; then
  printf 'packaging: verified cargo package -p horizon-browser unexpectedly succeeded; protocol is not on crates.io\n' >&2
  status=1
elif ! grep -q 'no matching package named `horizon-browser-protocol`' /tmp/horizon-browser-package-verify.log; then
  printf 'packaging: verified engine package failed for an unexpected reason:\n' >&2
  cat /tmp/horizon-browser-package-verify.log >&2
  status=1
fi

if [[ "$status" -ne 0 ]]; then
  exit 1
fi

printf 'browser packaging checks passed\n'
