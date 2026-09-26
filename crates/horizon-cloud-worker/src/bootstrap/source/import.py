"""Private worker helper, embedded in the binary. No checkout, hooks or network."""
import fcntl
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import resource
import select
import stat
import subprocess
import sys
import tarfile
import tempfile
import time

resource.setrlimit(resource.RLIMIT_AS, (2 * 1024**3, 2 * 1024**3))
resource.setrlimit(resource.RLIMIT_FSIZE, (8 * 1024**3, 8 * 1024**3))
os.umask(0o077)
# Keep allocation fencing alive through every helper and Git process.
LEASE = os.dup(0)
MAX_MATERIAL_BYTES = 4 * 1024**3
MAX_GIT_OUTPUT = 16 * 1024**2
MAX_TREE_OUTPUT = MAX_GIT_OUTPUT + 65536 * 64
GIT_TIMEOUT = 60
CONFIG = b"[core]\n\trepositoryformatversion = 0\n\tbare = true\n"


def require(value):
    if not value:
        raise ValueError("Invalid retained source material")


def safe(value):
    require(isinstance(value, str) and value and "\x00" not in value)
    path = PurePosixPath(value)
    require(not path.is_absolute() and str(path) == value)
    require(all(p not in (".", "..", ".git") for p in path.parts))
    return value


def directory(path):
    try:
        path.mkdir(mode=0o700)
    except FileExistsError:
        pass
    meta = path.lstat()
    require(stat.S_ISDIR(meta.st_mode) and meta.st_uid == os.geteuid() and not meta.st_mode & 0o077)


def immutable(path, content):
    ensure_stream(path, io.BytesIO(content), len(content))


def git_command(repo, args, index):
    command = ["/usr/bin/git", "--no-replace-objects", "-c", "core.hooksPath=/dev/null",
               "-c", "protocol.allow=never", "--git-dir=" + str(repo), *args]
    environment = dict(os.environ)
    if index is not None:
        environment['GIT_INDEX_FILE'] = index
    return command, environment


def git(repo, args, data=None, source=None, prefix=None, index=None):
    command, environment = git_command(repo, args, index)
    if prefix is not None:
        require(data is None and source is None and args[:2] == ["cat-file", "blob"])
        return git_prefix(command, environment, prefix)
    limit = MAX_TREE_OUTPUT if args[0] == "ls-tree" else MAX_GIT_OUTPUT
    with os.fdopen(os.memfd_create("source-git-output", os.MFD_CLOEXEC | os.MFD_ALLOW_SEALING), "w+b") as output:
        output.truncate(limit + 1)
        fcntl.fcntl(output, fcntl.F_ADD_SEALS, fcntl.F_SEAL_GROW | fcntl.F_SEAL_SHRINK)
        subprocess.run(command, input=data, stdin=source, stdout=output, stderr=subprocess.DEVNULL,
                       check=True, timeout=GIT_TIMEOUT, env=environment, pass_fds=(LEASE,))
        length = output.tell()
        require(length <= limit)
        output.seek(0)
        return output.read(length)


def git_attributes(repo, paths, index):
    command, environment = git_command(repo, ["check-attr", "--cached", "-z", "--stdin", "filter"], index)
    result, record, field, offset, matches = set(), 0, 0, 0, True
    with tempfile.TemporaryFile() as input:
        for path in paths:
            encoded = path.encode() + b"\0"
            require(input.tell() + len(encoded) <= MAX_TREE_OUTPUT)
            input.write(encoded)
        input.seek(0)
        with subprocess.Popen(command, stdin=input, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                              env=environment, pass_fds=(LEASE,)) as child:
            deadline = time.monotonic() + GIT_TIMEOUT
            try:
                while True:
                    remaining = deadline - time.monotonic()
                    require(remaining > 0 and select.select([child.stdout], [], [], remaining)[0])
                    block = os.read(child.stdout.fileno(), 65536)
                    if not block:
                        require(child.wait(timeout=max(0, deadline - time.monotonic())) == 0)
                        break
                    pieces = block.split(b"\0")
                    for position, piece in enumerate(pieces):
                        if not piece and position == len(pieces) - 1:
                            continue
                        require(record < len(paths))
                        expected = (paths[record].encode(), b"filter", b"lfs")[field]
                        matches = matches and expected[offset:offset + len(piece)] == piece
                        offset += len(piece)
                        if position < len(pieces) - 1:
                            equal = matches and offset == len(expected)
                            if field < 2:
                                require(equal)
                                field += 1
                            else:
                                if equal:
                                    result.add(paths[record])
                                record, field = record + 1, 0
                            offset, matches = 0, True
                require(record == len(paths) and field == 0 and offset == 0)
                return result
            finally:
                if child.poll() is None:
                    child.kill()
                child.wait()


def git_prefix(command, environment, limit):
    require(type(limit) is int and 0 < limit <= MAX_GIT_OUTPUT)
    with subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                          env=environment, pass_fds=(LEASE,)) as child:
        deadline = time.monotonic() + GIT_TIMEOUT
        result = bytearray()
        try:
            while len(result) < limit:
                remaining = deadline - time.monotonic()
                require(remaining > 0 and select.select([child.stdout], [], [], remaining)[0])
                block = os.read(child.stdout.fileno(), min(65536, limit - len(result)))
                if not block:
                    require(child.wait(timeout=max(0, deadline - time.monotonic())) == 0)
                    return bytes(result)
                result.extend(block)
            # Only the requested prefix is evidence; the rest is deliberately unread.
            return bytes(result)
        finally:
            if child.poll() is None:
                child.kill()
            child.wait()


def repository_layout(path, pack):
    with pack.open("rb") as stream:
        stream.seek(-20, os.SEEK_END)
        oid = stream.read().hex()
    allowed = {".": {"config", "HEAD", "objects", "refs"},
               "objects": {"pack", "info"}, "objects/info": set(),
               "objects/pack": {f"pack-{oid}.{suffix}" for suffix in ("pack", "idx", "rev")},
               "refs": {"heads"}, "refs/heads": {"base"}}
    for root, directories, files in os.walk(path, followlinks=False):
        relative = str(Path(root).relative_to(path))
        require(relative in allowed and set(directories + files) <= allowed[relative])
        for name in directories + files:
            meta = (Path(root) / name).lstat()
            require(stat.S_ISDIR(meta.st_mode) or stat.S_ISREG(meta.st_mode) and meta.st_nlink == 1)


def repository(path, pack, revision):
    require(len(revision) == 40 and all(c in "0123456789abcdef" for c in revision))
    directory(path)
    for name in ("objects", "refs", "refs/heads"):
        directory(path / name)
    repository_layout(path, pack)
    immutable(path / "config", CONFIG)
    immutable(path / "HEAD", b"ref: refs/heads/base\n")
    # Never consult a transferred config, template, alternates file or replace ref.
    require(not (path / "objects/info/alternates").exists())
    require(not (path / "refs/replace").exists())
    with pack.open("rb") as stream:
        git(path, ["index-pack", "--strict", "--stdin"], source=stream)
    require(git(path, ["cat-file", "-t", revision]).strip() == b"commit")
    immutable(path / "refs/heads/base", (revision + "\n").encode())
    git(path, ["fsck", "--strict", "--no-reflogs"])
    repository_layout(path, pack)


def archive():
    with tarfile.open("material.tar", mode="r:") as source:
        members = {}
        expanded = 0
        for index, member in enumerate(source):
            require(index < 20000 and (member.isfile() or member.isdir()) and not member.issparse())
            expanded += member.size
            require(expanded <= MAX_MATERIAL_BYTES)
            name = member.name.removeprefix("./").rstrip("/")
            if name in ("", "."):
                require(member.isdir() and "." not in members)
                name = "."
            else:
                safe(name)
            require(name not in members and member.size <= 4 * 1024**3)
            members[name] = member
        manifest = members.get("manifest.json")
        require(manifest is not None and manifest.isfile() and manifest.size <= 1024**2)
        value = json.load(source.extractfile(manifest))
        require(set(value) == {"modules", "assets"})
        require(len(value["modules"]) <= 256 and len(value["assets"]) <= 8192)
        expected = {"manifest.json", "lfs", "."}
        module_paths, asset_paths = set(), set()
        for index, module in enumerate(value["modules"]):
            require(set(module) == {"path", "revision"})
            path = safe(module["path"])
            require(path not in module_paths)
            module_paths.add(path)
            expected.add(f"module-{index}.pack")
        for asset in value["assets"]:
            require(set(asset) == {"path", "oid", "size"})
            path = safe(asset["path"])
            require(path not in asset_paths and path not in module_paths)
            asset_paths.add(path)
            oid = asset["oid"]
            require(isinstance(oid, str) and len(oid) == 64 and all(c in "0123456789abcdef" for c in oid))
            require(type(asset["size"]) is int and 0 <= asset["size"] <= 4 * 1024**3)
            expected.add("lfs/" + oid)
        require(set(members) == expected and members["lfs"].isdir())
        directory(Path("material"))
        directory(Path("material/lfs"))
        for name, member in members.items():
            if member.isdir():
                require(name in (".", "lfs"))
                continue
            target = Path("material") / name
            # Extraction is streamed into exact flat names; tar never controls
            # link creation, path traversal, permissions or filesystem calls.
            with source.extractfile(member) as stream:
                ensure_stream(target, stream, member.size)
        for asset in value["assets"]:
            path = Path("material/lfs") / asset["oid"]
            require(path.stat().st_size == asset["size"] and digest_file(path) == asset["oid"])
        return value


def ensure_stream(path, stream, length):
    flags = os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW
    with os.fdopen(os.open(path, flags, 0o600), "r+b") as output:
        meta = os.fstat(output.fileno())
        require(stat.S_ISREG(meta.st_mode) and meta.st_nlink == 1 and meta.st_size <= length)
        position = 0
        while position < length:
            block = stream.read(min(65536, length - position))
            require(block)
            overlap = max(0, min(len(block), meta.st_size - position))
            require(output.read(overlap) == block[:overlap])
            output.write(block[overlap:])
            position += len(block)
        require(not stream.read(1))
        output.flush()
        os.fsync(output.fileno())


def selected_material(manifest, revision):
    modules = {m["path"]: (i, m["revision"]) for i, m in enumerate(manifest["modules"])}
    found_modules, found_assets = set(), {}
    queue = [("", Path("repository.git"), revision, 0)]
    while queue:
        prefix, repo, selected, depth = queue.pop()
        require(depth <= 16)
        entries = git(repo, ["ls-tree", "-rz", "--full-tree", selected]).split(b"\0")[:-1]
        blobs = []
        for entry in entries:
            metadata, encoded = entry.split(b"\t", 1)
            mode, kind, oid = metadata.decode().split(" ")
            path = safe(encoded.decode())
            full = safe(prefix + path)
            if mode == "160000":
                require(full in modules and modules[full][1] == oid and full not in found_modules)
                found_modules.add(full)
                index, pinned = modules[full]
                child = Path(f"module-{index}.git")
                repository(child, Path(f"material/module-{index}.pack"), pinned)
                queue.append((full + "/", child, pinned, depth + 1))
            elif kind == "blob" and mode != "120000":
                blobs.append((path, full, oid))
        with tempfile.TemporaryDirectory() as scratch:
            index = scratch + "/index"
            git(repo, ["read-tree", selected], index=index)
            attributes = git_attributes(repo, [path for path, _, _ in blobs], index)
        for path, full, oid in blobs:
            if path not in attributes:
                continue
            pointer = git(repo, ["cat-file", "blob", oid], prefix=1025)
            if not pointer.startswith(b"version https://git-lfs.github.com/spec/v1\n"):
                continue
            require(len(pointer) <= 1024)
            lines = pointer.decode().splitlines()
            require(len(lines) == 3 and lines[0] == "version https://git-lfs.github.com/spec/v1")
            require(lines[1].startswith("oid sha256:") and lines[2].startswith("size "))
            found_assets[full] = (lines[1][11:], int(lines[2][5:]))
    require(found_modules == set(modules))
    require(found_assets == {a["path"]: (a["oid"], a["size"]) for a in manifest["assets"]})


def digest_file(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def inventory():
    digest = hashlib.sha256()
    total, count = 0, 0
    for root, directories, files in os.walk(".", followlinks=False):
        directories.sort()
        files.sort()
        for name in directories + files:
            path = Path(root) / name
            meta = path.lstat()
            require(meta.st_uid == os.geteuid() and not meta.st_mode & 0o077)
            require(stat.S_ISDIR(meta.st_mode) or stat.S_ISREG(meta.st_mode) and meta.st_nlink == 1)
            digest.update(str(path).encode() + b"\0")
            count += 1
            require(count <= 200000)
            if stat.S_ISREG(meta.st_mode):
                total += meta.st_size
                require(total <= 8 * 1024**3)
                digest.update(str(meta.st_size).encode() + b"\0" + digest_file(path).encode())
            descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
            try:
                os.fsync(descriptor)
            finally:
                os.close(descriptor)
    descriptor = os.open(".", os.O_RDONLY | os.O_DIRECTORY)
    os.fsync(descriptor)
    os.close(descriptor)
    return digest.hexdigest()


if sys.argv[1] == "build":
    material = archive()
    repository(Path("repository.git"), Path("pack"), sys.argv[2])
    selected_material(material, sys.argv[2])
sys.stdout.write(inventory())
