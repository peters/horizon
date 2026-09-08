#!/usr/bin/env python3
"""Audit the actual Docker context filter and optional final image layers."""

import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile


CRATES = ("horizon-repository", "horizon-core", "horizon-browser", "horizon-browser-protocol")
WORKER_FILES = {"Dockerfile", "build-tmux.sh", "entrypoint.sh", "host-identity.py", "rust-path.sh",
                "session.sh", "panel-session.py", "tmux.conf", "sshd_config"}


def execute(argv, **kwargs):
    return subprocess.run(argv, check=True, capture_output=True, timeout=600, **kwargs).stdout


def admitted(path):
    parts = path.parts
    return (str(path) in ("Cargo.toml", "Cargo.lock")
            or len(parts) == 3 and parts[0] == "crates" and parts[2] == "Cargo.toml"
            or len(parts) >= 4 and parts[0] == "crates" and parts[1] in CRATES
            and parts[2] == "src" and path.suffix == ".rs"
            or len(parts) == 3 and parts[:2] == ("containers", "remote-worker") and parts[2] in WORKER_FILES)


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


def image_test(docker, image, fixture, expected_binary):
    identity = json.loads(execute(docker + ["image", "inspect", image]))[0]["Id"]
    archive = fixture / "image.tar"
    execute(docker + ["image", "save", "--output", str(archive), identity])
    binary_hashes = []
    layers = 0
    with tarfile.open(archive) as outer:
        manifests = json.load(outer.extractfile("manifest.json"))
        assert len(manifests) == 1
        for layer_name in manifests[0]["Layers"]:
            with tarfile.open(fileobj=outer.extractfile(layer_name), mode="r|*") as layer:
                for item in layer:
                    name = item.name.removeprefix("./")
                    assert not name.startswith("opt/horizon-repository-build/"), name
                    assert not name.startswith("opt/horizon-manifests/"), name
                    if name.startswith("opt/horizon-dependency-cache/crates/"):
                        assert not item.isfile() or name.endswith("/Cargo.toml"), name
                    if name == "usr/local/bin/horizon-repository":
                        assert item.isfile() and item.mode & 0o111
                        binary_hashes.append(hashlib.sha256(layer.extractfile(item).read()).hexdigest())
            layers += 1
    assert binary_hashes == [expected_binary], binary_hashes
    print(f"PASS {layers} final image layers: no Horizon build/source tree; exactly the expected executable hash", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--docker-host", required=True)
    parser.add_argument("--image")
    parser.add_argument("--expected-binary-sha256")
    options = parser.parse_args()
    if not options.docker_host.startswith("unix:///"):
        parser.error("an explicit local Unix Docker socket is required")
    if options.image and (not options.expected_binary_sha256 or len(options.expected_binary_sha256) != 64):
        parser.error("image audit requires the separately observed build-stage binary SHA-256")
    docker = ["docker", "--host", options.docker_host]
    root = Path(__file__).resolve().parents[2]
    with tempfile.TemporaryDirectory(prefix="horizon-repository-packaging-") as temporary:
        fixture = Path(temporary)
        context_test(docker, root, fixture)
        if options.image:
            image_test(docker, options.image, fixture, options.expected_binary_sha256)
    print("Removed only temporary audit context, export and archive; images unchanged", flush=True)


if __name__ == "__main__":
    main()
