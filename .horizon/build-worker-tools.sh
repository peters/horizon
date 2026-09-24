#!/usr/bin/env bash
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y --no-install-recommends build-essential pkg-config libssl-dev libxkbcommon-dev git ca-certificates python3
export CARGO_HOME=/tmp/helper-cargo RUSTUP_TOOLCHAIN=1.98.1 GIT_LFS_SKIP_SMUDGE=1
git init /tmp/horizon-source
git -C /tmp/horizon-source remote add origin https://github.com/peters/horizon.git
git -C /tmp/horizon-source fetch --depth 1 origin d9e3774685f4f13addd3146f4e5287bd33182f07
git -C /tmp/horizon-source checkout --detach FETCH_HEAD
cd /tmp/horizon-source
cargo build --locked --release --jobs 8 -p horizon-cloud-worker -p horizon-browser-cli -p horizon-device --features horizon-device/cli
mkdir -p /output/bin /output/share/licenses/horizon
for binary in horizon-cloud-worker horizon-browser horizon-device; do
    install -m 755 "target/release/$binary" /output/bin/
    strip "/output/bin/$binary"
done
install -m 755 examples/cloud-worker/horizon-worker-* /output/bin/
install -m 644 LICENSE /output/share/licenses/horizon/LICENSE
printf '%s\n' 'https://github.com/peters/horizon/tree/d9e3774685f4f13addd3146f4e5287bd33182f07' > /output/share/licenses/horizon/SOURCE
