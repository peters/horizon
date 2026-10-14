"""Private host control files, anchored to no-follow directory descriptors."""
from contextlib import contextmanager
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
