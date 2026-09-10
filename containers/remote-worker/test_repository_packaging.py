#!/usr/bin/env python3
"""Audit the actual Docker context filter and optional final image layers."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tempfile


CRATES = ("horizon-repository", "horizon-core", "horizon-browser", "horizon-browser-protocol")
WORKER_FILES = {"Dockerfile", "build-tmux.sh", "entrypoint.sh", "host-identity.py", "rust-path.sh",
                "session.sh", "panel-session.py", "setup-launch.py", "tmux.conf", "sshd_config"}
AGENT_PATHS = tuple("usr/local/bin/" + tool for tool in
                    ("codex", "claude", "gemini", "opencode", "kilo", "pi", "grok")) + tuple(
    "usr/local/lib/node_modules/" + package for package in
    ("@openai/codex", "@anthropic-ai/claude-code", "@google/gemini-cli", "opencode-ai",
     "@kilocode/cli", "@earendil-works/pi-coding-agent"))


def agent_member(name):
    """Match shipped agent paths, including whiteouts that hide their ancestors."""
    name = name.removeprefix("./")
    parent, _, leaf = name.rpartition("/")
    whiteout = leaf.startswith(".wh.")
    if whiteout:
        name = parent if leaf == ".wh..wh..opq" else (parent + "/" if parent else "") + leaf[4:]
    return any(name == path or name.startswith(path + "/")
               or whiteout and (not name or path.startswith(name + "/")) for path in AGENT_PATHS)


def agent_path_test():
    for path in AGENT_PATHS:
        parent, _, leaf = path.rpartition("/")
        for member in (path, "./" + path, path + "/child", parent + "/.wh." + leaf):
            assert agent_member(member), member
        for member in (path + "-unrelated", "other/" + path):
            assert not agent_member(member), member
    assert agent_member("usr/local/lib/node_modules/.wh.@openai")
    assert agent_member("usr/local/bin/.wh..wh..opq")
    assert agent_member(".wh.usr")
    assert agent_member(".wh..wh..opq")
    assert not agent_member("usr/local/lib/node_modules/npm/package.json")
    assert not agent_member("usr/local/bin/.wh.node")
    assert not agent_member("etc/.wh.ssh")


def execute(argv, **kwargs):
    return subprocess.run(argv, check=True, capture_output=True, timeout=600, **kwargs).stdout


def admitted(path):
    parts = path.parts
    return (str(path) in ("Cargo.toml", "Cargo.lock", "LICENSE")
            or len(parts) == 3 and parts[0] == "crates" and parts[2] == "Cargo.toml"
            or len(parts) >= 4 and parts[0] == "crates" and parts[1] in CRATES
            and parts[2] == "src" and path.suffix == ".rs"
            or len(parts) == 3 and parts[:2] == ("containers", "remote-worker") and parts[2] in WORKER_FILES)


def flavor_test(root):
    # Execute the real RUN bodies with no executable search path. Only the
    # explicitly mocked tool functions can run; no downloads or installs occur.
    agent_path_test()
    dockerfile = (root / "containers/remote-worker/Dockerfile").read_text()
    lines = dockerfile.replace("\\\n", "").splitlines()
    arguments = dict(line[4:].split("=", 1) for line in lines if line.startswith("ARG ") and "=" in line)
    assert arguments["WORKER_AGENT_TOOLS"] == "full"
    assert sum(line.startswith("ARG WORKER_AGENT_TOOLS") for line in lines) == 1
    runs = [line[4:] for line in lines if line.startswith("RUN ")]
    validation = next(command for command in runs if command.startswith('case "${WORKER_AGENT_TOOLS}"'))
    gated = [command for command in runs if command.startswith('if [ "${WORKER_AGENT_TOOLS}" = full ]')]
    assert len(gated) == 3
    assert runs.index(validation) < min(runs.index(command) for command in gated)
    tools = ("npm", "curl", "sha256sum", "chmod", "codex", "claude", "gemini", "opencode", "kilo", "pi", "grok")
    mocks = "\n".join(
        f'{tool}() {{ printf "%s\\n" "{tool} $*"; [ "${{FAIL_TOOL-}}" != "{tool}" ]; }}'
        for tool in tools)

    def run(command, flavor, fail="", arch="amd64"):
        return subprocess.run(["/bin/sh", "-c", mocks + "\n" + command],
                              env=dict(arguments, PATH="/nonexistent", WORKER_AGENT_TOOLS=flavor,
                                       FAIL_TOOL=fail, TARGETARCH=arch),
                              stdin=subprocess.DEVNULL, capture_output=True, timeout=10, check=False)

    for flavor in ("full", "shell"):
        assert run(validation, flavor).returncode == 0
    for flavor in ("", "Shell", "minimal", "full shell", "$(false)"):
        result = run(validation, flavor)
        assert result.returncode == 64 and b"must be full or shell" in result.stderr
    for command in gated:
        result = run(command, "shell")
        assert result.returncode == 0 and not result.stdout and not result.stderr
        result = run(command, arguments["WORKER_AGENT_TOOLS"])
        assert result.returncode == 0 and result.stdout and not result.stderr
    npm, grok, versions = gated
    npm_output = run(npm, "full").stdout
    for package in ("@openai/codex@", "@anthropic-ai/claude-code@", "@google/gemini-cli@",
                    "opencode-ai@", "@kilocode/cli@", "@earendil-works/pi-coding-agent@"):
        assert package.encode() in npm_output
    assert b"--strict-allow-scripts" in npm_output and b"npm cache clean --force" in npm_output
    assert run(npm, "full", fail="npm").returncode != 0
    for arch, suffix in (("amd64", "x86_64"), ("arm64", "aarch64")):
        result = run(grok, "full", arch=arch)
        assert result.returncode == 0 and f"-linux-{suffix}".encode() in result.stdout
        assert b"sha256sum --check --strict -" in result.stdout
    assert run(grok, "full", arch="invalid").returncode == 64
    for tool in ("curl", "sha256sum", "chmod"):
        assert run(grok, "full", fail=tool).returncode != 0
    assert run(versions, "full").stdout.splitlines() == [f"{tool} --version".encode() for tool in tools[4:]]
    for tool in tools[4:]:
        assert run(versions, "full", fail=tool).returncode != 0
    apt = next(command for command in runs if "openssh-server" in command)
    assert "&& rm -f /etc/ssh/ssh_host_*" in apt
    assert "COPY LICENSE /usr/local/share/licenses/horizon/LICENSE" in lines
    assert "COPY --from=node-runtime /usr/local/LICENSE /usr/local/share/licenses/node/LICENSE" in lines
    assert "COPY --from=tmux-runtime /opt/horizon-tmux/share/licenses/tmux/COPYING /usr/local/share/licenses/tmux/COPYING" in lines
    assert 'install -D -m 0644 COPYING "${tmux_prefix}/share/licenses/tmux/COPYING"' in (root / "containers/remote-worker/build-tmux.sh").read_text()
    assert "!LICENSE" in (root / ".dockerignore").read_text().splitlines()
    print("PASS static flavor contract: full default, shell omissions, invalid values, tool failures, notices and same-layer key removal", flush=True)


def context_test(docker, root, fixture):
    context, exported = fixture / "context", fixture / "exported"
    context.mkdir()
    tracked = [Path(p.decode()) for p in execute(["git", "-C", str(root), "ls-files", "-z"]).split(b"\0") if p]
    expected = {}
    for path in tracked:
        # Include unrelated Rust source as a negative control without copying LFS assets.
        if admitted(path) or path.suffix == ".rs" or str(path) == ".dockerignore":
            target = context / path
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(root / path, target)
            if admitted(path):
                expected[str(path)] = hashlib.sha256(target.read_bytes()).hexdigest()
    for name in (".env", ".git/config", ".cargo/config.toml", ".ssh/id_ed25519", "target/fixture.rs",
                 "crates/horizon-core/src/fixture.env", "crates/horizon-core/src/private/key.pem",
                 "crates/horizon-core/src/fixture.rs/private.env", "crates/horizon-core/src/fixture.rs/nested/key.pem",
                 "crates/horizon-core/src/fixture.rs/nested/source.rs",
                 "crates/horizon-core/src/.git/hooks/fixture.rs",
                 "crates/horizon-browser/src/nested/.git/hooks/fixture.rs",
                 "crates/horizon-ui/src/untracked.rs", "containers/remote-worker/fixture-token"):
        sentinel = context / name
        sentinel.parent.mkdir(parents=True, exist_ok=True)
        sentinel.write_text("synthetic excluded marker\n")
    execute(docker + ["build", "--file", "-", "--output", f"type=local,dest={exported}", str(context)],
            input=b"FROM scratch\nCOPY . /\n")
    observed = {str(path.relative_to(exported)): hashlib.sha256(path.read_bytes()).hexdigest()
                for path in exported.rglob("*") if path.is_file()}
    assert observed == expected, (observed.keys() - expected.keys(), expected.keys() - observed.keys())
    assert all(any(name.startswith(f"crates/{crate}/src/") for name in observed) for crate in CRATES)
    print(f"PASS actual Docker filter: {len(observed)} exact source/manifest/worker inputs; unrelated source and sentinels excluded", flush=True)


def image_test(docker, image, fixture, expected_binary, license_bytes, flavor, tmux_notice_hash, node_notice_hash):
    metadata = json.loads(execute(docker + ["image", "inspect", image]))[0]
    assert metadata["Config"]["Labels"]["io.horizon.worker.agent-tools"] == flavor
    identity = metadata["Id"]
    archive = fixture / "image.tar"
    execute(docker + ["image", "save", "--output", str(archive), identity])
    binary_hashes = []
    notices = {}
    agent_paths = set()
    layers = 0
    with tarfile.open(archive) as outer:
        manifests = json.load(outer.extractfile("manifest.json"))
        assert len(manifests) == 1
        for layer_name in manifests[0]["Layers"]:
            with tarfile.open(fileobj=outer.extractfile(layer_name), mode="r|*") as layer:
                for item in layer:
                    name = item.name.removeprefix("./")
                    if flavor == "shell":
                        assert not agent_member(name), name
                    elif name in AGENT_PATHS:
                        assert item.isfile() or item.isdir() or item.issym(), name
                        agent_paths.add(name)
                    assert not name.startswith("opt/horizon-repository-build/"), name
                    assert not name.startswith("opt/horizon-manifests/"), name
                    assert not name.startswith("etc/ssh/ssh_host_"), name
                    if name.startswith("opt/horizon-dependency-cache/crates/"):
                        assert not item.isfile() or name.endswith("/Cargo.toml"), name
                    if name == "usr/local/bin/horizon-repository":
                        assert item.isfile() and item.mode & 0o111
                        binary_hashes.append(hashlib.sha256(layer.extractfile(item).read()).hexdigest())
                    if name in ("usr/local/share/licenses/horizon/LICENSE", "usr/local/share/licenses/tmux/COPYING",
                                "usr/local/share/licenses/node/LICENSE"):
                        limit = 1024 * 1024 if name == "usr/local/share/licenses/node/LICENSE" else 16384
                        assert item.isfile() and 0 < item.size <= limit and name not in notices, name
                        notices[name] = layer.extractfile(item).read()
            layers += 1
    assert binary_hashes == [expected_binary], binary_hashes
    if flavor == "full":
        assert agent_paths == set(AGENT_PATHS), set(AGENT_PATHS) - agent_paths
    assert notices["usr/local/share/licenses/horizon/LICENSE"] == license_bytes
    assert hashlib.sha256(notices["usr/local/share/licenses/tmux/COPYING"]).hexdigest() == tmux_notice_hash
    assert hashlib.sha256(notices["usr/local/share/licenses/node/LICENSE"]).hexdigest() == node_notice_hash
    print(f"PASS {layers} final image layers: {flavor} agent paths, no Horizon build/source tree or SSH host keys; expected executable and exact notices", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--docker-host")
    parser.add_argument("--static-only", action="store_true")
    parser.add_argument("--image")
    parser.add_argument("--expected-binary-sha256")
    parser.add_argument("--expected-agent-tools", choices=("full", "shell"))
    parser.add_argument("--expected-tmux-notice-sha256")
    parser.add_argument("--expected-node-notice-sha256")
    options = parser.parse_args()
    expected = (options.expected_binary_sha256, options.expected_agent_tools,
                options.expected_tmux_notice_sha256, options.expected_node_notice_sha256)
    if options.static_only and (options.image or any(expected)):
        parser.error("static-only mode does not audit an image")
    if not options.static_only and (not options.docker_host or not options.docker_host.startswith("unix:///")):
        parser.error("an explicit local Unix Docker socket is required")
    if options.image and (not options.expected_agent_tools or any(
            not value or not re.fullmatch("[0-9a-f]{64}", value)
            for value in (options.expected_binary_sha256, options.expected_tmux_notice_sha256,
                          options.expected_node_notice_sha256))):
        parser.error("image audit requires expected flavor and separately observed binary/tmux/Node notice SHA-256 values")
    if not options.image and any(expected):
        parser.error("expected image values require --image")
    root = Path(__file__).resolve().parents[2]
    flavor_test(root)
    if options.static_only:
        return
    docker = ["docker", "--host", options.docker_host]
    with tempfile.TemporaryDirectory(prefix="horizon-repository-packaging-") as temporary:
        fixture = Path(temporary)
        context_test(docker, root, fixture)
        if options.image:
            image_test(docker, options.image, fixture, options.expected_binary_sha256,
                       (root / "LICENSE").read_bytes(), options.expected_agent_tools,
                       options.expected_tmux_notice_sha256, options.expected_node_notice_sha256)
    print("Removed only temporary audit context, export and archive; images unchanged", flush=True)


if __name__ == "__main__":
    main()
