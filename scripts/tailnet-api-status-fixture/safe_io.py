"""Private host control files, anchored to no-follow directory descriptors."""
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import stat
import uuid


def _refused():
    return RuntimeError("private fixture file refused")


def _regular(descriptor, limit=None):
    metadata = os.fstat(descriptor)
    if (not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.getuid()
            or metadata.st_mode & 0o077 or metadata.st_nlink != 1
            or (limit is not None and metadata.st_size > limit)):
        raise _refused()
    return metadata


def _identity(metadata):
    return metadata.st_dev, metadata.st_ino


class PrivateDirectory:
    """Keep the private directory inode even if its pathname is swapped later."""
    def __init__(self, path):
        path = Path(path).absolute()
        if ".." in path.parts:
            raise _refused()
        descriptor = os.open("/", os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
        try:
            for component in path.parts[1:]:
                child = os.open(component, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC,
                                dir_fd=descriptor)
                os.close(descriptor)
                descriptor = child
            metadata = os.fstat(descriptor)
            if metadata.st_uid != os.getuid() or metadata.st_mode & 0o077:
                raise _refused()
        except OSError as error:
            os.close(descriptor)
            raise _refused() from error
        except BaseException:
            os.close(descriptor)
            raise
        self.descriptor = descriptor

    def __enter__(self):
        return self

    def __exit__(self, *_):
        os.close(self.descriptor)

    @staticmethod
    def _name(name):
        if not isinstance(name, str) or name in ("", ".", "..") or "/" in name:
            raise _refused()
        return name

    def _open(self, name, flags):
        try:
            return os.open(self._name(name), flags | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC,
                           0o600, dir_fd=self.descriptor)
        except (FileNotFoundError, FileExistsError):
            raise
        except OSError as error:
            raise _refused() from error

    def read_bytes(self, name, limit):
        if not isinstance(limit, int) or limit < 0:
            raise _refused()
        descriptor = self._open(name, os.O_RDONLY)
        try:
            before = _regular(descriptor, limit)
            chunks, length = [], 0
            while length <= limit:
                chunk = os.read(descriptor, min(65536, limit + 1 - length))
                if not chunk:
                    break
                chunks.append(chunk)
                length += len(chunk)
            after = _regular(descriptor, limit)
            if length > limit or _identity(before) != _identity(after):
                raise _refused()
            return b"".join(chunks)
        finally:
            os.close(descriptor)

    def read_json(self, name, limit):
        def unique(pairs):
            value = {}
            for key, item in pairs:
                if key in value:
                    raise _refused()
                value[key] = item
            return value
        try:
            return json.loads(self.read_bytes(name, limit), object_pairs_hook=unique)
        except (ValueError, UnicodeError) as error:
            raise _refused() from error

    @contextmanager
    def open_exclusive(self, name):
        descriptor = self._open(name, os.O_WRONLY | os.O_CREAT | os.O_EXCL)
        try:
            _regular(descriptor)
            output = os.fdopen(descriptor, "wb")
        except BaseException:
            os.close(descriptor)
            raise
        with output:
            yield output

    def write_exclusive_json(self, name, value):
        with self.open_exclusive(name) as output:
            output.write((json.dumps(value) + "\n").encode())
            output.flush()
            os.fsync(output.fileno())
            identity = _identity(_regular(output.fileno()))
        os.fsync(self.descriptor)
        return identity

    def unlink_owned(self, name, identity):
        descriptor = self._open(name, os.O_RDONLY)
        try:
            if _identity(_regular(descriptor)) != identity:
                raise _refused()
            current = os.stat(name, dir_fd=self.descriptor, follow_symlinks=False)
            if _identity(current) != identity or not stat.S_ISREG(current.st_mode) or current.st_nlink != 1:
                raise _refused()
            os.unlink(name, dir_fd=self.descriptor)
            os.fsync(self.descriptor)
        finally:
            os.close(descriptor)

    def replace_json(self, name, value):
        """Replace host-only journals without truncating any previous inode."""
        name = self._name(name)
        try:
            prior = self._open(name, os.O_RDONLY)
        except FileNotFoundError:
            previous = None
        else:
            try:
                previous = _identity(_regular(prior))
            finally:
                os.close(prior)
        temporary = ".fixture-write-" + uuid.uuid4().hex
        self.write_exclusive_json(temporary, value)
        try:
            try:
                current = os.stat(name, dir_fd=self.descriptor, follow_symlinks=False)
            except FileNotFoundError:
                if previous is not None:
                    raise _refused()
            else:
                if (previous is None or _identity(current) != previous or not stat.S_ISREG(current.st_mode)
                        or current.st_nlink != 1 or current.st_uid != os.getuid() or current.st_mode & 0o077):
                    raise _refused()
            os.replace(temporary, name, src_dir_fd=self.descriptor, dst_dir_fd=self.descriptor)
            os.fsync(self.descriptor)
        finally:
            try:
                os.unlink(temporary, dir_fd=self.descriptor)
            except FileNotFoundError:
                pass


def read_bytes(path, limit):
    path = Path(path)
    with PrivateDirectory(path.parent) as directory:
        return directory.read_bytes(path.name, limit)


def read_json(path, limit):
    path = Path(path)
    with PrivateDirectory(path.parent) as directory:
        return directory.read_json(path.name, limit)


def write_exclusive_json(path, value):
    path = Path(path)
    with PrivateDirectory(path.parent) as directory:
        return directory.write_exclusive_json(path.name, value)


@contextmanager
def open_exclusive(path):
    path = Path(path)
    with PrivateDirectory(path.parent) as directory, directory.open_exclusive(path.name) as output:
        yield output


def replace_json(path, value):
    path = Path(path)
    with PrivateDirectory(path.parent) as directory:
        directory.replace_json(path.name, value)


def _public_regular(descriptor, limit):
    metadata = os.fstat(descriptor)
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_nlink != 1 or metadata.st_size > limit:
        raise RuntimeError("qualification input file refused")
    return metadata


def _stamp(metadata):
    return (metadata.st_dev, metadata.st_ino, metadata.st_size,
            metadata.st_mtime_ns, metadata.st_ctime_ns)


@contextmanager
def public_input(path, limit):
    """Open a bounded public build input once, without following any symlink."""
    path = Path(path).absolute()
    if not isinstance(limit, int) or limit < 0 or ".." in path.parts:
        raise RuntimeError("qualification input path refused")
    directory = os.open("/", os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    descriptor = None
    try:
        try:
            for component in path.parts[1:-1]:
                child = os.open(component, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC,
                                dir_fd=directory)
                os.close(directory)
                directory = child
            descriptor = os.open(path.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC,
                                 dir_fd=directory)
            _public_regular(descriptor, limit)
        except OSError as error:
            raise RuntimeError("qualification input path refused") from error
        yield descriptor
    finally:
        if descriptor is not None:
            os.close(descriptor)
        os.close(directory)


def _chunks(descriptor, limit):
    length = 0
    while length <= limit:
        chunk = os.read(descriptor, min(65536, limit + 1 - length))
        if not chunk:
            return
        length += len(chunk)
        if length > limit:
            raise RuntimeError("qualification input oversized")
        yield chunk


def read_public_bytes(path, limit):
    with public_input(path, limit) as descriptor:
        before = _stamp(_public_regular(descriptor, limit))
        value = b"".join(_chunks(descriptor, limit))
        if before != _stamp(_public_regular(descriptor, limit)) or len(value) != before[2]:
            raise RuntimeError("qualification input changed while reading")
        return value


def copy_verified(path, target, limit, expected=None, mode=0o400):
    """Hash and copy the same source FD; verify the exclusive frozen FD before use."""
    target = Path(target)
    with public_input(path, limit) as source:
        before = _stamp(_public_regular(source, limit))
        digest = hashlib.sha256()
        for chunk in _chunks(source, limit):
            digest.update(chunk)
        accepted = digest.hexdigest()
        if before != _stamp(_public_regular(source, limit)):
            raise RuntimeError("qualification input changed while hashing")
        if expected is not None and accepted != expected:
            raise RuntimeError("frozen artifact hash mismatch")
        os.lseek(source, 0, os.SEEK_SET)
        with PrivateDirectory(target.parent) as directory:
            descriptor = directory._open(target.name, os.O_RDWR | os.O_CREAT | os.O_EXCL)
            with os.fdopen(descriptor, "w+b") as output:
                identity = _identity(_regular(output.fileno()))
                copied = hashlib.sha256()
                for chunk in _chunks(source, limit):
                    output.write(chunk)
                    copied.update(chunk)
                output.flush()
                output.seek(0)
                frozen = hashlib.sha256()
                for chunk in _chunks(output.fileno(), limit):
                    frozen.update(chunk)
                current = os.stat(target.name, dir_fd=directory.descriptor, follow_symlinks=False)
                if (before != _stamp(_public_regular(source, limit)) or copied.hexdigest() != accepted
                        or frozen.hexdigest() != accepted or _identity(current) != identity
                        or _identity(_regular(output.fileno(), limit)) != identity):
                    raise RuntimeError("frozen qualification input changed")
                os.fchmod(output.fileno(), mode)
                os.fsync(output.fileno())
            os.fsync(directory.descriptor)
        return accepted
