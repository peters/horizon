#!/usr/bin/env python3
"""Retain one SSH host identity on trusted workspace storage before serving SSH."""

import base64
import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import time

LIMIT = 16 * 1024
KEYGEN_TIMEOUT_SECONDS = 10
INITIALIZATION_SYNC_GRACE_SECONDS = 10
# Generation and public-key inspection both finish before the readiness marker.
WAIT_SECONDS = 2 * KEYGEN_TIMEOUT_SECONDS + INITIALIZATION_SYNC_GRACE_SECONDS
KEY_NAME = "ssh_host_ed25519_key"
ED25519_PREFIX = b"\x00\x00\x00\x0bssh-ed25519\x00\x00\x00\x20"


class IdentityError(Exception):
    """No sensitive path, key material or subprocess diagnostics in errors."""


def fail():
    raise IdentityError("retained SSH host identity is unavailable")


def trusted_directory(path, private=False):
    if not path.is_absolute() or ".." in path.parts:
        fail()
    for directory in (*reversed(path.parents), path):
        info = directory.lstat()
        if not stat.S_ISDIR(info.st_mode) or info.st_uid not in (0, os.geteuid()):
            fail()
        if info.st_mode & 0o022 and not info.st_mode & stat.S_ISVTX:
            fail()
    info = path.lstat()
    if private and (info.st_uid != os.geteuid() or info.st_mode & 0o077):
        fail()


def read_file(path, private=True):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, "rb") as stream:
        info = os.fstat(stream.fileno())
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.geteuid()
                or (private and info.st_mode & 0o077) or not 0 < info.st_size <= LIMIT):
            fail()
        value = stream.read(LIMIT + 1)
        if not 0 < len(value) <= LIMIT:
            fail()
        return value


def public_key(value):
    text = value.decode("utf-8").rstrip("\n")
    if any(ord(character) < 32 and character != "\t" for character in text):
        fail()
    fields = text.split()
    if len(fields) < 2 or fields[0] != "ssh-ed25519":
        fail()
    payload = base64.b64decode(fields[1], validate=True)
    if len(payload) != len(ED25519_PREFIX) + 32 or not payload.startswith(ED25519_PREFIX):
        fail()
    return " ".join(fields[:2])


def keygen(*arguments):
    result = subprocess.run(
        ["/usr/bin/ssh-keygen", *map(str, arguments)], check=False, timeout=KEYGEN_TIMEOUT_SECONDS,
        stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
        env={"PATH": "/usr/bin:/bin", "LC_ALL": "C"},
    )
    if result.returncode != 0 or len(result.stdout) > LIMIT:
        fail()
    return result.stdout


def synchronize(path, directory=False):
    flags = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK
    if directory:
        flags |= os.O_DIRECTORY
    descriptor = os.open(path, flags)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def publish(path, value):
    with tempfile.NamedTemporaryFile(prefix=".identity-", dir=path.parent) as candidate:
        candidate.write(value)
        candidate.flush()
        os.fsync(candidate.fileno())
        os.link(candidate.name, path, follow_symlinks=False)
        synchronize(path.parent, directory=True)


def unique_object(fields):
    result = {}
    for key, value in fields:
        if key in result:
            fail()
        result[key] = value
    return result


class HostIdentity:
    def __init__(self, workspace, access_key, runtime_directory):
        self.workspace = Path(workspace)
        self.access_key = Path(access_key)
        self.runtime_directory = Path(runtime_directory)
        self.parent = self.workspace / ".horizon-worker"
        self.directory = self.parent / "ssh"
        self.claim = self.parent / "ssh.claim"
        self.marker = self.directory / "ready.json"

    def prepare(self):
        trusted_directory(self.workspace)
        trusted_directory(self.access_key.parent)
        trusted_directory(self.runtime_directory)
        access_digest = hashlib.sha256(public_key(read_file(self.access_key)).encode()).hexdigest()
        self.parent.mkdir(mode=0o700, exist_ok=True)
        trusted_directory(self.parent, private=True)
        synchronize(self.workspace, directory=True)
        if os.path.lexists(self.directory) and not os.path.lexists(self.claim):
            fail()
        try:
            publish(self.claim, access_digest.encode())
        except FileExistsError:
            created = False
        else:
            created = True
            self.directory.mkdir(mode=0o700)
        if read_file(self.claim) != access_digest.encode():
            fail()
        synchronize(self.parent, directory=True)
        if created:
            trusted_directory(self.directory, private=True)
            self.initialize(access_digest)
        host_public_key = self.load(access_digest)
        self.materialize(host_public_key)
        return host_public_key

    def initialize(self, access_digest):
        # An existing runtime key needs an explicit migration, never replacement.
        for suffix in ("", ".pub"):
            if os.path.lexists(self.runtime_directory / (KEY_NAME + suffix)):
                fail()
        private_path = self.directory / KEY_NAME
        keygen("-q", "-t", "ed25519", "-N", "", "-C", "", "-f", private_path)
        for suffix in ("", ".pub"):
            path = self.directory / (KEY_NAME + suffix)
            path.chmod(0o600)
            read_file(path)
            synchronize(path)
        host_public_key = public_key(keygen("-y", "-P", "", "-f", private_path))
        marker = {"version": 1, "access_digest": access_digest, "host_public_key": host_public_key}
        publish(self.marker, json.dumps(marker, sort_keys=True).encode())

    def load(self, access_digest):
        deadline = time.monotonic() + WAIT_SECONDS
        while True:
            try:
                trusted_directory(self.directory, private=True)
                encoded = read_file(self.marker)
                break
            except FileNotFoundError:
                if time.monotonic() >= deadline:
                    fail()
                time.sleep(0.02)
        marker = json.loads(encoded, object_pairs_hook=unique_object)
        if (not isinstance(marker, dict)
                or set(marker) != {"version", "access_digest", "host_public_key"}
                or type(marker["version"]) is not int or marker["version"] != 1
                or marker["access_digest"] != access_digest
                or not isinstance(marker["host_public_key"], str)):
            fail()
        private_path = self.directory / KEY_NAME
        read_file(private_path)
        expected = marker["host_public_key"]
        if (public_key(read_file(self.directory / (KEY_NAME + ".pub"))) != expected
                or public_key(keygen("-y", "-P", "", "-f", private_path)) != expected):
            fail()
        synchronize(self.directory, directory=True)
        return expected

    def materialize(self, host_public_key):
        private_path = self.runtime_directory / KEY_NAME
        public_path = self.runtime_directory / (KEY_NAME + ".pub")
        private_value = read_file(self.directory / KEY_NAME)
        for path, value in ((private_path, private_value), (public_path, (host_public_key + "\n").encode())):
            try:
                publish(path, value)
            except FileExistsError:
                if read_file(path) != value:
                    fail()
            synchronize(path)
        synchronize(self.runtime_directory, directory=True)


def main():
    try:
        if len(sys.argv) != 1:
            fail()
        HostIdentity("/workspace", "/root/.ssh/authorized_keys", "/etc/ssh").prepare()
    except (IdentityError, OSError, ValueError, RecursionError, subprocess.SubprocessError):
        print("horizon-worker: retained SSH host identity is unavailable", file=sys.stderr)
        return 64
    return 0


if __name__ == "__main__":
    sys.exit(main())
