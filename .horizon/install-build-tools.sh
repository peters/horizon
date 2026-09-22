#!/usr/bin/env bash
set -euo pipefail
mode="${1:-}"
case "$mode" in cpu|gpu) ;; *) exit 2;; esac
export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y --no-install-recommends ca-certificates curl python3
mkdir -p /etc/apt/keyrings
curl -fsSL https://packages.mozilla.org/apt/repo-signing-key.gpg -o /etc/apt/keyrings/mozilla.asc
printf '%s\n' 'deb [signed-by=/etc/apt/keyrings/mozilla.asc] https://packages.mozilla.org/apt mozilla main' > /etc/apt/sources.list.d/mozilla.list
printf 'Package: firefox*\nPin: origin packages.mozilla.org\nPin-Priority: 1000\n' > /etc/apt/preferences.d/mozilla
apt-get update
apt-get install -y --no-install-recommends \
    openssh-server tmux git git-lfs gh tini util-linux \
    build-essential pkg-config cmake clang libssl-dev \
    libxkbcommon-dev libwayland-dev libxcb-render0-dev libxcb-shape0-dev \
    libxcb-xfixes0-dev libvulkan-dev libgl-dev libasound2-dev \
    mesa-vulkan-drivers vulkan-tools glslc \
    xvfb openbox x11vnc xauth x11-utils libxtst6 fonts-dejavu-core \
    bubblewrap dbus-daemon xinput xdotool x11-xserver-utils ffmpeg \
    firefox /tmp/browser-packages/*.deb
rm -rf /var/lib/apt/lists/* /tmp/browser-packages
rm -f /etc/ssh/ssh_host_*
: > /etc/machine-id
npm install -g @openai/codex@0.155.1
npm install -g @anthropic-ai/claude-code@2.1.278
npm cache clean --force
python3 /usr/local/lib/horizon/retain-component-sources.py
mkdir -p /usr/local/share/licenses/agent-client
for notice in LICENSE NOTICE; do
    curl -fsSL "https://raw.githubusercontent.com/openai/codex/be2951ea34f0d295ed0becf97079f92fa5f6950e/$notice" \
        -o "/usr/local/share/licenses/agent-client/$notice"
done
printf '%s\n' 'https://github.com/openai/codex/tree/be2951ea34f0d295ed0becf97079f92fa5f6950e' \
    > /usr/local/share/licenses/agent-client/SOURCE
mkdir -p /usr/local/share/licenses/agent-client/bundled
curl -fsSL https://raw.githubusercontent.com/openai/codex/be2951ea34f0d295ed0becf97079f92fa5f6950e/codex-rs/vendor/bubblewrap/COPYING -o /usr/local/share/licenses/agent-client/bundled/agent-bubblewrap-COPYING
printf '%s\n' 'b7993225104d90ddd8024fd838faf300bea5e83d91203eab98e29512acebd69c  /usr/local/share/licenses/agent-client/bundled/agent-bubblewrap-COPYING' | sha256sum -c -
curl -fsSL https://raw.githubusercontent.com/openai/codex/be2951ea34f0d295ed0becf97079f92fa5f6950e/third_party/wezterm/LICENSE -o /usr/local/share/licenses/agent-client/bundled/agent-wezterm-LICENSE
printf '%s\n' '331312c214f14dc1455a0e45e3a66f4b70bbc73916cffd525570e8cdb9d63bf4  /usr/local/share/licenses/agent-client/bundled/agent-wezterm-LICENSE' | sha256sum -c -
curl -fsSL https://raw.githubusercontent.com/BurntSushi/ripgrep/15.2.0/COPYING -o /usr/local/share/licenses/agent-client/bundled/rg-COPYING
printf '%s\n' '01c266bced4a434da0051174d6bee16a4c82cf634e2679b6155d40d75012390f  /usr/local/share/licenses/agent-client/bundled/rg-COPYING' | sha256sum -c -
curl -fsSL https://raw.githubusercontent.com/BurntSushi/ripgrep/15.2.0/LICENSE-MIT -o /usr/local/share/licenses/agent-client/bundled/rg-LICENSE-MIT
printf '%s\n' '0f96a83840e146e43c0ec96a22ec1f392e0680e6c1226e6f3ba87e0740af850f  /usr/local/share/licenses/agent-client/bundled/rg-LICENSE-MIT' | sha256sum -c -
curl -fsSL https://raw.githubusercontent.com/BurntSushi/ripgrep/15.2.0/UNLICENSE -o /usr/local/share/licenses/agent-client/bundled/rg-UNLICENSE
printf '%s\n' '7e12e5df4bae12cb21581ba157ced20e1986a0508dd10d0e8a4ab9a4cf94e85c  /usr/local/share/licenses/agent-client/bundled/rg-UNLICENSE' | sha256sum -c -
curl -fsSL https://raw.githubusercontent.com/zsh-users/zsh/77045ef899e53b9598bebc5a41db93a548a40ca6/LICENCE -o /usr/local/share/licenses/agent-client/bundled/zsh-LICENCE
printf '%s\n' 'd06fdf3ef9b1ec69d6b9e170b0a9516fbad3523261ff1668bde3bfea6e0ef5f5  /usr/local/share/licenses/agent-client/bundled/zsh-LICENCE' | sha256sum -c -
curl -fsSL https://github.com/mozilla/geckodriver/releases/download/v0.36.0/geckodriver-v0.36.0-linux64.tar.gz -o /tmp/geckodriver.tar.gz
printf '%s\n' '0bde38707eb0a686a20c6bd50f4adcc7d60d4f73c60eb83ee9e0db8f65823e04  /tmp/geckodriver.tar.gz' | sha256sum -c -
tar -xzf /tmp/geckodriver.tar.gz -C /usr/local/bin
rm /tmp/geckodriver.tar.gz
mkdir -p /usr/local/share/licenses/geckodriver
curl -fsSL https://raw.githubusercontent.com/mozilla/geckodriver/v0.36.0/LICENSE -o /usr/local/share/licenses/geckodriver/LICENSE
CARGO_HOME=/usr/local/cargo rustup component add rustfmt clippy
mkdir -p /etc/horizon-worker /run/sshd /workspace
printf '%s\n' '{"agents":["codex","claude"],"browsers":["chromium","firefox"],"desktop":true}' > /etc/horizon-worker/capabilities.json
ln -s /usr/local/bin/horizon-worker-git-auth /usr/local/bin/gh
printf 'AuthorizedKeysFile /run/sshd/horizon-authorized-keys\nPasswordAuthentication no\nPermitRootLogin prohibit-password\nPermitUserEnvironment no\n' > /etc/ssh/sshd_config.d/horizon.conf
if [ "$mode" = gpu ]; then nvcc --version; fi
rustc --version
cargo --version
chromium --version
firefox --version
horizon-worker-check --git-auth
