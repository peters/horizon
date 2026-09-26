"""Bounded, committed-only materialization into one previously empty private tree.

Git reads only immutable imported repositories. All writes, including indexes,
are made here; no checkout, filter, hooks, templates or network command runs.
"""
import fcntl
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import resource
import select
import stat
import struct
import subprocess
import sys
import time

MAX_TOTAL = 8 * 1024**3
MAX_WORKING = 4 * 1024**3
MAX_METADATA = 64 * 1024**2
MAX_OUTPUT = 20 * 1024**2
resource.setrlimit(resource.RLIMIT_AS, (2 * 1024**3, 2 * 1024**3))
os.umask(0o077)
LEASE = os.dup(0)
DEADLINE = time.monotonic() + 600
SOURCE = Path.cwd()
DESTINATION = Path(sys.argv[1])
MODE, REVISION, SESSION, PROJECT = sys.argv[2:]
os.chdir(DESTINATION)
DESTINATION = Path(".")


def require(value):
    if not value:
        raise ValueError("Invalid or exhausted session preparation")


def remaining():
    left = DEADLINE - time.monotonic()
    require(left > 0)
    return min(60, left)


def safe(value):
    require(isinstance(value, str) and value and "\x00" not in value)
    path = PurePosixPath(value)
    require(not path.is_absolute() and str(path) == value)
    require(all(p not in (".", "..", ".git") for p in path.parts))
    require(len(value.encode()) <= 4096)
    return value


class Budget:
    def __init__(self):
        self.data = 0
        self.working = 0
        # Reserve control-record capacity outside the helper's tree as well.
        self.metadata = 1024**2

    def charge(self, size, kind):
        require(type(size) is int and size >= 0)
        if kind == "metadata":
            self.metadata += size
            require(self.metadata <= MAX_METADATA)
        else:
            self.data += size
            require(self.data <= MAX_TOTAL - MAX_METADATA)
            if kind == "working":
                self.working += size
                require(self.working <= MAX_WORKING)


BUDGET = Budget()


def directory(path):
    remaining()
    path.mkdir(mode=0o700)


def parents(path):
    # Only traverse the task's freshly created tree, never committed symlinks.
    relative = path.relative_to(DESTINATION)
    current = DESTINATION
    for part in relative.parts:
        current = current / part
        try:
            directory(current)
        except FileExistsError:
            require(stat.S_ISDIR(current.lstat().st_mode))


def write(path, stream, length, kind, executable=False):
    remaining()
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW
    with os.fdopen(os.open(path, flags, 0o700 if executable else 0o600), "wb") as output:
        left = length
        while left:
            remaining()
            block = stream.read(min(left, 65536))
            require(block and len(block) <= left)
            BUDGET.charge(len(block), kind)
            output.write(block)
            left -= len(block)
        require(not stream.read(1))
        output.flush()
        os.fsync(output.fileno())


def content(path, value):
    write(path, io.BytesIO(value), len(value), "metadata")


def copy(source, destination, kind):
    descriptor = os.open(source, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, "rb") as stream:
        meta = os.fstat(stream.fileno())
        require(stat.S_ISREG(meta.st_mode) and meta.st_nlink == 1)
        write(destination, stream, meta.st_size, kind)


def command(repo, args):
    return ["/usr/bin/git", "--no-replace-objects", "-c", "core.hooksPath=/dev/null",
            "-c", "protocol.allow=never", "--git-dir=" + str(repo), *args]


def git(repo, args):
    require(args[0] in ("ls-tree", "cat-file"))
    with os.fdopen(os.memfd_create("session-metadata", os.MFD_CLOEXEC | os.MFD_ALLOW_SEALING), "w+b") as output:
        output.truncate(MAX_OUTPUT + 1)
        fcntl.fcntl(output, fcntl.F_ADD_SEALS, fcntl.F_SEAL_GROW | fcntl.F_SEAL_SHRINK)
        subprocess.run(command(repo, args), stdin=subprocess.DEVNULL, stdout=output, stderr=subprocess.DEVNULL,
                       check=True, timeout=remaining(), pass_fds=(LEASE,))
        length = output.tell()
        require(length <= MAX_OUTPUT)
        output.seek(0)
        return output.read(length)


def blob(repo, oid, destination, length, executable):
    # Stream and verify exactly the preflight size under a per-command deadline.
    with subprocess.Popen(command(repo, ["cat-file", "blob", oid]), stdin=subprocess.DEVNULL,
                          stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, pass_fds=(LEASE,)) as child:
        deadline = time.monotonic() + remaining()
        class Reader:
            def read(self, size):
                left = deadline - time.monotonic()
                require(left > 0 and select.select([child.stdout], [], [], left)[0])
                return os.read(child.stdout.fileno(), size)
        try:
            write(destination, Reader(), length, "working", executable)
            require(child.wait(timeout=max(0, deadline - time.monotonic())) == 0)
        finally:
            if child.poll() is None:
                child.kill()
            child.wait()


def plan(material):
    modules = {m["path"]: (index, m["revision"]) for index, m in enumerate(material["modules"])}
    assets = {a["path"]: a for a in material["assets"]}
    require(len(modules) <= 256 and len(assets) <= 8192)
    queue = [("", SOURCE / "repository.git", REVISION, 0)]
    plans, found, used_assets = [], set(), set()
    entry_count, path_bytes, working, copies = 0, 0, 0, 0
    seen_paths = set()
    while queue:
        prefix, repo, revision, depth = queue.pop()
        require(depth <= 16)
        entries = []
        output = git(repo, ["ls-tree", "-rlz", "--full-tree", revision])
        for raw in output.split(b"\0")[:-1]:
            metadata, encoded = raw.split(b"\t", 1)
            mode, kind, oid, size = metadata.decode().split()
            path = safe(encoded.decode())
            full = safe(prefix + path)
            for component in (PurePosixPath(full), *PurePosixPath(full).parents):
                value = str(component)
                if value != "." and value not in seen_paths:
                    seen_paths.add(value)
                    entry_count += 1
                    path_bytes += len(value.encode())
            require(entry_count <= 65536 and path_bytes <= 16 * 1024**2)
            if mode == "160000":
                require(kind == "commit" and full in modules and full not in found)
                index, pinned = modules[full]
                require(pinned == oid)
                found.add(full)
                queue.append((full + "/", SOURCE / f"module-{index}.git", pinned, depth + 1))
                length = 0
            else:
                require(kind == "blob" and mode in ("100644", "100755", "120000"))
                length = int(size)
                require(length >= 0)
                if mode == "120000":
                    require(length <= 4095 and full not in assets)
                if full in assets:
                    used_assets.add(full)
                    length = assets[full]["size"]
                    # Each cache copy is separate from the hydrated working file.
                    copies += length
                working += length
                require(working <= MAX_WORKING)
            entries.append((path, mode, oid, length, assets.get(full)))
        object_files = []
        for root, dirs, files in os.walk(repo / "objects", followlinks=False):
            for name in dirs:
                require(stat.S_ISDIR((Path(root) / name).lstat().st_mode))
            for name in files:
                item = Path(root) / name
                meta = item.lstat()
                require(stat.S_ISREG(meta.st_mode) and meta.st_nlink == 1)
                require(item.parent.name == "pack" and name.startswith("pack-") and item.suffix in (".pack", ".idx", ".rev"))
                copies += meta.st_size
                object_files.append(item)
        require(working + copies <= MAX_TOTAL - MAX_METADATA)
        plans.append((prefix, repo, revision, entries, object_files))
    require(found == set(modules) and used_assets == set(assets))
    require(os.statvfs(DESTINATION).f_bavail * os.statvfs(DESTINATION).f_frsize >= working + copies + MAX_METADATA)
    return plans


def index_entry(path, mode, oid, target):
    # Index v2: stat cache, mode, object ID, flags and NUL-padded pathname.
    meta = target.lstat()
    encoded = path.encode()
    fields = [meta.st_ctime_ns // 10**9, meta.st_ctime_ns % 10**9,
              meta.st_mtime_ns // 10**9, meta.st_mtime_ns % 10**9,
              meta.st_dev, meta.st_ino, int(mode, 8), meta.st_uid, meta.st_gid, meta.st_size]
    value = struct.pack("!10I", *(field & 0xffffffff for field in fields))
    value += bytes.fromhex(oid) + struct.pack("!H", min(len(encoded), 0xfff)) + encoded + b"\0"
    return value + b"\0" * ((-len(value)) % 8)


def prepare_repository(item):
    prefix, repo, revision, entries, object_files = item
    checkout = DESTINATION / "checkout" / prefix.rstrip("/")
    parents(checkout)
    metadata = checkout / ".git"
    directory(metadata)
    for name in ("objects", "objects/pack", "objects/info", "refs", "refs/heads", "refs/heads/projects", f"refs/heads/projects/{PROJECT}", "lfs", "lfs/objects"):
        directory(metadata / name)
    content(metadata / "config", b"[core]\n\trepositoryformatversion = 0\n\tbare = false\n\tfilemode = true\n")
    content(metadata / "HEAD", f"ref: refs/heads/projects/{PROJECT}/{SESSION}\n".encode())
    content(metadata / f"refs/heads/projects/{PROJECT}" / SESSION, (revision + "\n").encode())
    for source in object_files:
        copy(source, metadata / "objects/pack" / source.name, "data")
    index = bytearray(b"DIRC" + struct.pack("!II", 2, len(entries)))
    for path, mode, oid, length, asset in sorted(entries, key=lambda entry: entry[0].encode()):
        target = checkout / path
        parents(target.parent)
        if mode == "160000":
            parents(target)
        elif mode == "120000":
            value = git(repo, ["cat-file", "blob", oid])
            require(len(value) == length and b"\0" not in value)
            BUDGET.charge(len(value), "working")
            os.symlink(os.fsdecode(value), target)
        elif asset:
            source = SOURCE / "material/lfs" / asset["oid"]
            copy(source, target, "working")
            cache = metadata / "lfs/objects" / asset["oid"][:2] / asset["oid"][2:4]
            parents(cache)
            cached = cache / asset["oid"]
            if not cached.exists():
                copy(source, cached, "data")
            if mode == "100755":
                target.chmod(0o700)
        else:
            blob(repo, oid, target, length, mode == "100755")
        index.extend(index_entry(path, mode, oid, target))
        require(len(index) + BUDGET.metadata + 20 <= MAX_METADATA)
    index.extend(hashlib.sha1(index).digest())
    content(metadata / "index", index)


def sync_tree():
    # New staging only. Never called after the publication-authorized record.
    for root, dirs, files in os.walk(DESTINATION, topdown=False, followlinks=False):
        remaining()
        for name in files:
            path = Path(root) / name
            if path.is_symlink():
                continue
            descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
            try:
                os.fsync(descriptor)
            finally:
                os.close(descriptor)
        descriptor = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)


def inventory(sync=False):
    digest = hashlib.sha256()
    total, count = 0, 0
    for root, directories, files in os.walk(DESTINATION, followlinks=False):
        directories.sort()
        files.sort()
        for name in directories + files:
            remaining()
            path = Path(root) / name
            meta = path.lstat()
            count += 1
            require(count <= 300000 and meta.st_uid == os.geteuid())
            digest.update(str(path.relative_to(DESTINATION)).encode() + b"\0")
            digest.update(str(stat.S_IMODE(meta.st_mode)).encode() + b"\0")
            if stat.S_ISLNK(meta.st_mode):
                value = os.fsencode(os.readlink(path))
                total += len(value)
                digest.update(b"link\0" + value + b"\0")
            elif stat.S_ISREG(meta.st_mode):
                require(meta.st_nlink == 1 and not meta.st_mode & 0o077)
                total += meta.st_size
                require(total <= MAX_TOTAL)
                contents = hashlib.sha256()
                descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
                with os.fdopen(descriptor, "rb") as stream:
                    opened = os.fstat(stream.fileno())
                    require((opened.st_dev, opened.st_ino) == (meta.st_dev, meta.st_ino))
                    while block := stream.read(65536):
                        remaining()
                        contents.update(block)
                    if sync:
                        os.fsync(stream.fileno())
                digest.update(b"file\0" + contents.digest())
            else:
                require(stat.S_ISDIR(meta.st_mode) and not meta.st_mode & 0o077)
                digest.update(b"directory\0")
                if sync:
                    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
                    try:
                        opened = os.fstat(descriptor)
                        require((opened.st_dev, opened.st_ino) == (meta.st_dev, meta.st_ino))
                        os.fsync(descriptor)
                    finally:
                        os.close(descriptor)
    require(total <= MAX_TOTAL)
    if sync:
        descriptor = os.open(DESTINATION, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)
    return digest.hexdigest()


def main():
    require(len(REVISION) == 40 and all(c in "0123456789abcdef" for c in REVISION))
    require(len(PROJECT) == 36 and all(c in "0123456789abcdef-" for c in PROJECT))
    require(len(SESSION) == 36 and all(c in "0123456789abcdef-" for c in SESSION))
    require(not any(DESTINATION.iterdir()))
    material = json.loads((SOURCE / "material/manifest.json").read_bytes())
    plans = plan(material)
    for name in ("checkout", "home", "runtime", "logs", "tools"):
        directory(DESTINATION / name)
    # Parent repositories precede nested submodules regardless of manifest order.
    for item in sorted(plans, key=lambda item: item[0].count("/")):
        prepare_repository(item)
    sync_tree()
    sys.stdout.write(inventory())


if MODE == "build":
    main()
else:
    require(MODE == "verify")
    sys.stdout.write(inventory(sync=True))
