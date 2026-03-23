#!/usr/bin/env bash

set -euo pipefail

version_repo_root() {
  cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd
}

workspace_base_version() {
  local cargo_file
  cargo_file="$(version_repo_root)/Cargo.toml"

  awk '
    /^\[workspace\.package\]$/ { in_section = 1; next }
    /^\[/ { in_section = 0 }
    in_section && $1 == "version" {
      value = $3
      gsub(/"/, "", value)
      print value
      exit
    }
  ' "$cargo_file"
}

workspace_horizon_core_version() {
  local cargo_file
  cargo_file="$(version_repo_root)/Cargo.toml"

  awk '
    /^\[workspace\.dependencies\]$/ { in_section = 1; next }
    /^\[/ { in_section = 0 }
    in_section && $1 == "horizon-core" {
      if (match($0, /version[[:space:]]*=[[:space:]]*"[^"]+"/)) {
        value = substr($0, RSTART, RLENGTH)
        split(value, parts, "\"")
        print parts[2]
      }
      exit
    }
  ' "$cargo_file"
}

workspace_horizon_cursor_version() {
  local cargo_file
  cargo_file="$(version_repo_root)/Cargo.toml"

  awk '
    /^\[workspace\.dependencies\]$/ { in_section = 1; next }
    /^\[/ { in_section = 0 }
    in_section && $1 == "horizon-cursor" {
      if (match($0, /version[[:space:]]*=[[:space:]]*"[^"]+"/)) {
        value = substr($0, RSTART, RLENGTH)
        split(value, parts, "\"")
        print parts[2]
      }
      exit
    }
  ' "$cargo_file"
}

rewrite_workspace_versions() {
  local version="$1"
  local repo_root cargo_file temp_file

  repo_root="$(version_repo_root)"
  cargo_file="$repo_root/Cargo.toml"
  temp_file="$(mktemp)"

  awk -v version="$version" '
    /^\[workspace\.package\]$/ {
      in_workspace_package = 1
      in_workspace_dependencies = 0
      print
      next
    }
    /^\[workspace\.dependencies\]$/ {
      in_workspace_package = 0
      in_workspace_dependencies = 1
      print
      next
    }
    /^\[/ {
      in_workspace_package = 0
      in_workspace_dependencies = 0
    }
    in_workspace_package && $1 == "version" {
      print "version = \"" version "\""
      next
    }
    in_workspace_dependencies && $1 == "horizon-core" {
      print "horizon-core = { path = \"crates/horizon-core\", version = \"" version "\" }"
      next
    }
    in_workspace_dependencies && $1 == "horizon-cursor" {
      print "horizon-cursor = { path = \"crates/horizon-cursor\", version = \"" version "\" }"
      next
    }
    {
      print
    }
  ' "$cargo_file" > "$temp_file"

  mv "$temp_file" "$cargo_file"
}
