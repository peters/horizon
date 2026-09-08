#!/usr/bin/env python3
"""Exercise an existing local image against disposable synthetic retained storage."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import random
import shutil
import stat
import struct
import subprocess
import tempfile
import uuid


def execute(argv, **kwargs):
    return subprocess.run(argv, check=True, capture_output=True, timeout=300, **kwargs).stdout


def encoded(value):
    return json.dumps(value, separators=(",", ":"), ensure_ascii=False).encode()


def digest(data):
    return hashlib.sha256(data).hexdigest()


def snapshot(root):
    return {str(path.relative_to(root)): (digest(path.read_bytes()), stat.S_IMODE(path.stat().st_mode))
            for path in root.rglob("*") if path.is_file()}


class ImageSmoke:
    def __init__(self, options):
        if not options.docker_host.startswith("unix:///"):
            raise ValueError("an explicit local Unix Docker socket is required")
        self.docker = ["docker", "--host", options.docker_host]
        self.image = json.loads(execute(self.docker + ["image", "inspect", options.image]))[0]["Id"]
        security = json.loads(execute(self.docker + ["info", "--format", "{{json .SecurityOptions}}"] ))
        self.user = "0:0" if "name=rootless" in security else f"{os.geteuid()}:{os.getegid()}"
        self.label = str(uuid.uuid4())
        self.containers = []
        self.root = Path(tempfile.mkdtemp(prefix="horizon-repository-image-", dir=options.fixture_parent))
        self.identity = self.root.stat().st_dev, self.root.stat().st_ino
        self.environment = {"PATH": "/usr/bin:/bin", "LC_ALL": "C", "GIT_CONFIG_NOSYSTEM": "1",
                            "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_SYSTEM": "/dev/null",
                            "GIT_ALLOW_PROTOCOL": "", "GIT_TERMINAL_PROMPT": "0",
                            "GIT_AUTHOR_NAME": "Fixture", "GIT_AUTHOR_EMAIL": "fixture@example.invalid",
                            "GIT_COMMITTER_NAME": "Fixture", "GIT_COMMITTER_EMAIL": "fixture@example.invalid",
                            "GIT_AUTHOR_DATE": "2000-01-01T00:00:00Z", "GIT_COMMITTER_DATE": "2000-01-01T00:00:00Z"}

    def inspect(self, container):
        value = json.loads(execute(self.docker + ["container", "inspect", container]))[0]
        assert value["Id"] == container and value["Image"] == self.image
        assert value["Config"]["Labels"]["horizon.repository-image-smoke"] == self.label
        return value

    def command(self, entrypoint, args, data=b"", overlay=False):
        mounts = ["--mount", f"type=bind,src={self.root / 'source/objects'},dst=/objects,readonly",
                  "--mount", f"type=bind,src={self.root / 'bundles'},dst=/bundles,readonly"]
        if not overlay:
            mounts += ["--mount", f"type=bind,src={self.root / 'retained'},dst=/retained"]
        command = self.docker + ["create", "--pull=never", "--network=none", "--interactive",
                                "--user", self.user, "--label", f"horizon.repository-image-smoke={self.label}",
                                "--entrypoint", entrypoint] + mounts + [self.image] + args
        container = execute(command).decode().strip()
        self.containers.append(container)
        self.inspect(container)
        output = subprocess.run(self.docker + ["start", "--attach", "--interactive", container],
                                input=data, capture_output=True, timeout=300, check=False)
        observed = self.inspect(container)
        assert not observed["State"]["Running"] and not observed["State"]["OOMKilled"]
        assert output.returncode == observed["State"]["ExitCode"]
        return output

    def materialize(self, request, status, code):
        output = self.command("/usr/local/bin/horizon-repository", ["materialize"], request)
        assert output.returncode == code, (output.returncode, output.stdout, output.stderr)
        assert not output.stderr and output.stdout.endswith(b"\n") and len(output.stdout) <= 128 * 1024
        receipt = json.loads(output.stdout)
        assert receipt["version"] == 1 and receipt["status"] == status
        return receipt

    def retire_completed(self):
        while self.containers:
            container = self.containers[-1]
            assert not self.inspect(container)["State"]["Running"]
            execute(self.docker + ["container", "rm", container])
            self.containers.pop()

    def git(self, root, *args, data=None, check=True):
        return subprocess.run(["/usr/bin/git", "--no-replace-objects", "-C", str(root), *args],
                              input=data, env=self.environment, capture_output=True, timeout=120, check=check)

    def fixture(self):
        for name in ("source", "bundles", "retained"):
            (self.root / name).mkdir(mode=0o700)
        source = self.root / "source"
        self.git(source, "init", "--bare", "--template=", "--object-format=sha1")
        old_blob = self.git(source, "hash-object", "-w", "--stdin", data=b"unrelated old bytes").stdout.strip()
        old_tree = self.git(source, "mktree", data=b"100644 blob " + old_blob + b"\told\n").stdout.strip()
        ancestor = self.git(source, "commit-tree", old_tree.decode(), data=b"old\n").stdout.strip()
        self.large = random.Random(383).randbytes(65 * 1024 * 1024 + 1)
        self.pointer = b"version https://git-lfs.github.com/spec/v1\noid sha256:synthetic\nsize 99\n"
        files = {".gitattributes": b"* filter=lfs\n", "asset": self.pointer,
                 "kept": b"unchanged", "large": self.large, "removed": b"remove this"}
        records = []
        for name, content in sorted(files.items()):
            oid = self.git(source, "hash-object", "-w", "--stdin", data=content).stdout.strip()
            records.append(b"100644 blob " + oid + b"\t" + name.encode() + b"\n")
        tree = self.git(source, "mktree", data=b"".join(records)).stdout.strip()
        self.base = self.git(source, "commit-tree", tree.decode(), "-p", ancestor.decode(), data=b"base\n").stdout.decode().strip()
        self.git(source, "update-ref", "refs/heads/main", self.base)
        self.git(source, "repack", "-ad")
        assert list((source / "objects/pack").glob("*.pack"))
        self.ancestor = ancestor.decode()
        self.staged = b"staged\0binary\xff"
        working = b"working-only hydrated bytes"
        blobs = {digest(self.staged): self.staged, digest(working): working}
        metadata = encoded({"domain": "horizon.repository-overlay", "version": 1,
                            "repository": "synthetic/project", "commit": self.base, "branch": None,
                            "index": [{"path": "link", "kind": "symlink", "target": "staged"},
                                      {"path": "removed", "kind": "remove"},
                                      {"path": "staged", "kind": "file", "sha256": digest(self.staged),
                                       "bytes": len(self.staged), "executable": True}],
                            "working_tree": [{"path": "asset", "kind": "file", "sha256": digest(working),
                                              "bytes": len(working), "executable": False}]})
        self.manifest = digest(metadata)
        bundle = b"HZOVLY\0\x01" + struct.pack("<I", len(metadata)) + metadata + struct.pack("<I", len(blobs))
        for key, value in sorted(blobs.items()):
            bundle += key.encode() + struct.pack("<Q", len(value)) + value
        path = self.root / "bundles" / (self.manifest + ".hzov")
        path.write_bytes(bundle)
        path.chmod(0o600)
        self.request = {"version": 1, "objects_directory": "/objects", "bundle_store": "/bundles",
                        "bundle_manifest": self.manifest, "scratch_parent": "/retained", "destination": "published"}

    def verify_checkout(self, path):
        assert self.git(path, "rev-parse", "HEAD").stdout.decode().strip() == self.base
        assert self.git(path, "rev-list", "--count", "HEAD").stdout == b"1\n"
        assert self.git(path, "rev-parse", "--is-shallow-repository").stdout == b"true\n"
        assert not self.git(path, "fsck", "--full", "--strict").stdout
        assert self.git(path, "cat-file", "-e", self.ancestor, check=False).returncode != 0
        assert self.git(path, "show", ":asset").stdout == self.pointer
        assert self.git(path, "show", ":staged").stdout == self.staged
        assert self.git(path, "show", ":link").stdout == b"staged"
        index = self.git(path, "ls-files", "--stage").stdout.decode().splitlines()
        assert any(line.startswith("100755 ") and line.endswith("\tstaged") for line in index)
        assert any(line.startswith("120000 ") and line.endswith("\tlink") for line in index)
        assert not any(line.endswith("\tremoved") for line in index)
        assert (path / "asset").read_bytes() == b"working-only hydrated bytes"
        assert (path / "staged").read_bytes() == self.staged
        assert stat.S_IMODE((path / "staged").stat().st_mode) == 0o755
        assert (path / "link").readlink() == Path("staged")
        assert (path / "large").read_bytes() == self.large
        assert (path / "kept").read_bytes() == b"unchanged" and not (path / "removed").exists()

    def test(self):
        self.fixture()
        before = snapshot(self.root / "source"), snapshot(self.root / "bundles")
        for args in ([], ["unknown"], ["materialize", "extra"]):
            result = self.command("/usr/local/bin/horizon-repository", args)
            assert result.returncode == 2 and not result.stdout
        for invalid in (b"{}", b"private-input-marker", b" " * 16385):
            receipt = self.materialize(invalid, "rejected", 2)
            assert "private-input-marker" not in str(receipt)
        missing = dict(self.request, bundle_manifest="0" * 64)
        self.materialize(encoded(missing), "unpublished", 1)
        assert not list((self.root / "retained").iterdir())
        receipt = self.materialize(encoded(self.request), "published", 0)
        assert receipt["checkout"] == "/retained/published" and receipt["base_commit"] == self.base
        assert receipt["bundle_manifest"] == self.manifest and receipt["possible_destination"] is None
        assert receipt["reason"] is None
        metadata = Path(receipt["source_metadata"])
        assert metadata.parent == Path("/retained")
        assert (self.root / "retained" / metadata.name / "HEAD").is_file()
        self.verify_checkout(self.root / "retained/published")
        collision = self.materialize(encoded(self.request), "unpublished", 1)
        assert "destination already exists" in collision["reason"]
        retained = Path(collision["checkout"])
        assert retained.parent == Path("/retained") and retained.name != "published"
        self.verify_checkout(self.root / "retained" / retained.name)
        self.verify_checkout(self.root / "retained/published")
        self.retire_completed()
        # A fresh, non-creating observer proves data outlives container removal.
        observation = self.command("/usr/bin/sha256sum", ["/retained/published/large"])
        assert observation.returncode == 0 and observation.stdout.decode().split()[0] == digest(self.large)
        # A private directory on the container's writable overlay is not qualified storage.
        overlay = self.command("/usr/bin/python3", ["-c",
            "import os,subprocess,sys; os.mkdir('/tmp/private-smoke',0o700); "
            "r=subprocess.run(['/usr/local/bin/horizon-repository','materialize'],input=sys.stdin.buffer.read(),capture_output=True); "
            "sys.stdout.buffer.write(r.stdout); sys.stderr.buffer.write(r.stderr); sys.exit(r.returncode)"],
            encoded(dict(self.request, scratch_parent="/tmp/private-smoke")), overlay=True)
        assert overlay.returncode == 1, (overlay.returncode, overlay.stdout, overlay.stderr)
        rejected = json.loads(overlay.stdout)
        assert rejected["status"] == "unpublished" and "unsupported" in rejected["reason"]
        assert before == (snapshot(self.root / "source"), snapshot(self.root / "bundles"))
        print("PASS image helper: strict input, packed 65 MiB-plus raw checkout, shallow HEAD/index/LFS/link/modes, "
              "qualified publication, no overwrite, retained unpublished checkout, fresh-container observation, "
              "unqualified overlay rejection and unchanged read-only inputs", flush=True)

    def close(self):
        for container in self.containers:
            self.inspect(container)
            execute(self.docker + ["container", "rm", "--force", container])
        assert self.identity == (self.root.stat().st_dev, self.root.stat().st_ino)
        shutil.rmtree(self.root)
        print("Removed only this smoke's exact containers and synthetic fixture; image retained", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", required=True)
    parser.add_argument("--docker-host", required=True, help="explicit local unix:///path/to/docker.sock")
    parser.add_argument("--fixture-parent", default="/tmp", help="trusted local journaled ext4 parent")
    options = parser.parse_args()
    smoke = ImageSmoke(options)
    print(f"Task-owned fixture: {smoke.root}; label: {smoke.label}; image: {smoke.image}", flush=True)
    try:
        smoke.test()
    finally:
        smoke.close()


if __name__ == "__main__":
    main()
