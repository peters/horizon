#!/usr/bin/env bash
set -euo pipefail

require_ubuntu_ci_sources() {
  if [[ ! -f "$1" || ! -r "$1" || ! -s "$1" ]]; then
    printf 'Required Ubuntu runner source file is unavailable: %s\n' "$1" >&2
    return 1
  fi
}

install_ci_ubuntu_dependencies() {
  if [[ "${GITHUB_ACTIONS:-}" != true || "${RUNNER_OS:-}" != Linux ]]; then
    printf 'This installer is only for Linux GitHub Actions runners.\n' >&2
    return 2
  fi
  if (( $# == 0 )); then
    printf 'Provide at least one Ubuntu package name.\n' >&2
    return 2
  fi
  local package
  for package in "$@"; do
    if [[ ! "$package" =~ ^[a-z0-9]+(-[a-z0-9]+)*$ ]]; then
      printf 'Only plain Ubuntu package names are accepted.\n' >&2
      return 2
    fi
  done

  local sources=/etc/apt/sources.list.d/ubuntu.sources
  require_ubuntu_ci_sources "$sources" || return "$?"
  # Scope both commands without changing runner repositories or pruning their cached lists.
  local -a apt_options=(
    -o "Dir::Etc::sourcelist=$sources"
    -o Dir::Etc::sourceparts=/dev/null
    -o APT::Get::List-Cleanup=0
  )
  sudo -n apt-get "${apt_options[@]}" -o APT::Update::Error-Mode=any update || return "$?"
  sudo -n apt-get "${apt_options[@]}" install -y -- "$@"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  install_ci_ubuntu_dependencies "$@"
fi
