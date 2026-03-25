#!/usr/bin/env bash
set -euo pipefail

host_path=""

append_path() {
  local dir
  for dir in "$@"; do
    if [ -d "$dir" ]; then
      host_path="${host_path:+${host_path}:}${dir}"
    fi
  done
}

# Prefer host tools for SSH, Tailscale, git helpers, and xdg-open behavior.
append_path \
  /usr/local/sbin \
  /usr/local/bin \
  /usr/sbin \
  /usr/bin \
  /sbin \
  /bin \
  /var/lib/snapd/hostfs/usr/local/sbin \
  /var/lib/snapd/hostfs/usr/local/bin \
  /var/lib/snapd/hostfs/usr/sbin \
  /var/lib/snapd/hostfs/usr/bin \
  /var/lib/snapd/hostfs/sbin \
  /var/lib/snapd/hostfs/bin

if [ -n "$host_path" ]; then
  export PATH="${host_path}:${PATH:-}"
fi

exec "$SNAP/bin/horizon" "$@"
