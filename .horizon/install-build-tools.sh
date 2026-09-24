#!/usr/bin/env bash
set -euo pipefail
mode="${1:-}"

# Agents install in their own layers after the toolchain, each keyed on the release
# passed to it. The build fails unless the agent reports exactly that release.
install_agent() {
    local agent="$1" package="$2" version="$3"
    npm install -g "$package@$version"
    npm cache clean --force
    python3 - "$agent" "$package" "$version" <<'PY'
import json, pathlib, re, subprocess, sys
agent, package, version = sys.argv[1:]
installed = json.loads(pathlib.Path('/usr/local/lib/node_modules', package, 'package.json').read_text())['version']
reported = subprocess.run([agent, '--version'], check=True, stdout=subprocess.PIPE, text=True, timeout=60).stdout
# The release must stand alone: 1.0.4 does not match 1.0.41, 1.0.4-beta or the
# tail of another version such as 1.0.0+1.0.4.
if installed != version or not re.search(
        r'(?:^|[^0-9A-Za-z.+-])v?' + re.escape(version) + r'(?![0-9A-Za-z+-]|\.[0-9A-Za-z])', reported):
    sys.exit(f'{package} {installed} reports {reported.strip()!r}; expected {version}')
record = pathlib.Path('/etc/horizon-worker/agent-versions.json')
versions = json.loads(record.read_text()) if record.exists() else {}
versions[agent] = version
record.write_text(json.dumps(versions, sort_keys=True) + '\n')
print(f'Installed {agent} {version}')
PY
}

case "$mode" in
    cpu|gpu) ;;
    codex)
        # The retained sources choose the release: the requested one, or the reviewed pin.
        version="$(python3 /usr/local/lib/horizon/retain-component-sources.py "${2:-}")"
        install_agent codex @openai/codex "$version"
        exit 0;;
    claude)
        # Builds without a requested release keep the reviewed one, so they stay reproducible.
        install_agent claude @anthropic-ai/claude-code "${2:-2.1.278}"
        exit 0;;
    *) exit 2;;
esac
export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y --no-install-recommends ca-certificates curl python3
mkdir -p /etc/apt/keyrings
curl -fsSL https://packages.mozilla.org/apt/repo-signing-key.gpg -o /etc/apt/keyrings/mozilla.asc
printf '%s\n' 'deb [signed-by=/etc/apt/keyrings/mozilla.asc] https://packages.mozilla.org/apt mozilla main' > /etc/apt/sources.list.d/mozilla.list
printf 'Package: firefox*\nPin: origin packages.mozilla.org\nPin-Priority: 1000\n' > /etc/apt/preferences.d/mozilla
apt-get update
apt-get install -y --no-install-recommends \
    openssh-server tmux git git-lfs gh rsync tini util-linux \
    build-essential pkg-config cmake clang libssl-dev \
    libxkbcommon-dev libxkbcommon-x11-0 libwayland-dev libxcb-render0-dev libxcb-shape0-dev \
    libxcb-xfixes0-dev libvulkan-dev libgl-dev libasound2-dev \
    mesa-vulkan-drivers vulkan-tools glslc \
    xvfb openbox x11vnc xauth x11-utils libxtst6 fonts-dejavu-core \
    bubblewrap dbus-daemon xinput xdotool x11-xserver-utils ffmpeg \
    firefox /tmp/browser-packages/*.deb
rm -rf /var/lib/apt/lists/* /tmp/browser-packages
rm -f /etc/ssh/ssh_host_*
: > /etc/machine-id
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
