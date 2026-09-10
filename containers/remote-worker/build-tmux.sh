#!/usr/bin/env bash
set -euo pipefail

# One pinned worker/test dependency; never install over an existing prefix.
readonly tmux_version=3.7c
readonly tmux_sha256=7c60cae9a0e25288e2e24750aafc9e8800fc7fd4555e447e1b29ee4201cfb3bf
if (($# != 1)) || [[ "$1" != /* || "$1" == / || -e "$1" || -L "$1" ]]; then
  printf '%s\n' 'usage: build-tmux.sh <new absolute install prefix>' >&2
  exit 64
fi
readonly tmux_prefix=$1
tmux_build_root=$(mktemp -d /tmp/horizon-tmux-build.XXXXXX)
readonly tmux_build_root
case "${tmux_build_root}" in
  /tmp/horizon-tmux-build.*) ;;
  *) exit 64 ;;
esac
trap 'rm -rf -- "${tmux_build_root}"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP

curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location \
  "https://github.com/tmux/tmux/releases/download/${tmux_version}/tmux-${tmux_version}.tar.gz" \
  --output "${tmux_build_root}/source.tar.gz"
printf '%s  %s\n' "${tmux_sha256}" "${tmux_build_root}/source.tar.gz" | sha256sum --check --strict -
tar --extract --gzip --file "${tmux_build_root}/source.tar.gz" --directory "${tmux_build_root}" --no-same-owner
cd "${tmux_build_root}/tmux-${tmux_version}"
./configure --prefix="${tmux_prefix}" --disable-utempter
make -j2
mkdir -m 0755 -- "${tmux_prefix}"
mkdir -m 0755 -- "${tmux_prefix}/bin"
install -m 0755 tmux "${tmux_prefix}/bin/tmux"
install -D -m 0644 COPYING "${tmux_prefix}/share/licenses/tmux/COPYING"
test "$("${tmux_prefix}/bin/tmux" -V)" = "tmux ${tmux_version}"
