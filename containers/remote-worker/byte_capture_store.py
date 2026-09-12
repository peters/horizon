"""Private retained-volume records; never cleanup, export, or certify provider durability."""

import hashlib
import json
import os
import re
import secrets
import stat

LIMIT = 128 * 1024
CAPACITY = 1024 * 1024 * 1024
MAX_RECORDS = 4096
FLAGS = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC


class CaptureError(Exception):
    """Only static diagnostics are emitted across the CLI boundary."""


def require(condition):
    if not condition:
        raise CaptureError('capture state is unavailable; retain all data')


def unique(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result)
        result[key] = value
    return result


def decode(data):
    try:
        return json.loads(data, object_pairs_hook=unique)
    except (ValueError, RecursionError):
        raise CaptureError('invalid capture JSON') from None


def encode(value):
    return json.dumps(value, separators=(',', ':'), ensure_ascii=True).encode() + b'\n'


def digest(value):
    return isinstance(value, str) and re.fullmatch('[0-9a-f]{64}', value) is not None


def identity(info):
    # Reading must not fail merely because the filesystem updates access time.
    return tuple(getattr(info, field) for field in (
        'st_dev', 'st_ino', 'st_mode', 'st_uid', 'st_gid', 'st_nlink',
        'st_size', 'st_mtime_ns', 'st_ctime_ns'))


class Directory:
    def __init__(self, path):
        require(os.path.isabs(path) and os.path.normpath(path) == path)
        fd = os.open('/', FLAGS)
        try:
            for part in path.split('/')[1:]:
                require(part not in ('', '.', '..'))
                child = os.open(part, FLAGS, dir_fd=fd)
                os.close(fd)
                fd = child
            self.path, self.fd = path, fd
            self.original = os.fstat(fd)
            self.verify()
        except BaseException:
            os.close(fd)
            raise

    def close(self):
        os.close(self.fd)

    def verify(self):
        held, current = os.fstat(self.fd), os.stat(self.path, follow_symlinks=False)
        require(stat.S_ISDIR(held.st_mode) and held.st_uid == os.geteuid()
                and stat.S_IMODE(held.st_mode) == 0o700 and held.st_nlink > 0
                and (held.st_dev, held.st_ino) == (current.st_dev, current.st_ino)
                == (self.original.st_dev, self.original.st_ino))

    def child(self, name, create=False):
        require(re.fullmatch('[a-z0-9-]+', name) is not None)
        self.verify()
        if create:
            os.mkdir(name, 0o700, dir_fd=self.fd)
            os.fsync(self.fd)
        result = Directory(self.path + '/' + name)
        require(result.original.st_dev == self.original.st_dev)
        self.verify()
        return result

    def read(self, name, limit=LIMIT):
        self.verify()
        require('/' not in name and name not in ('.', '..'))
        try:
            fd = os.open(name, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=self.fd)
        except FileNotFoundError:
            return None
        try:
            before = os.fstat(fd)
            require(stat.S_ISREG(before.st_mode) and before.st_uid == os.geteuid()
                    and stat.S_IMODE(before.st_mode) == 0o600 and before.st_nlink == 1
                    and before.st_size <= limit)
            data = bytearray()
            while len(data) <= limit:
                block = os.read(fd, min(65536, limit + 1 - len(data)))
                if not block:
                    break
                data.extend(block)
            require(len(data) <= limit and identity(before) == identity(os.fstat(fd))
                    == identity(os.stat(name, dir_fd=self.fd, follow_symlinks=False)))
            self.verify()
            return bytes(data)
        finally:
            os.close(fd)

    def write(self, name, value, replace=False):
        data = encode(value)
        require(len(data) <= LIMIT and '/' not in name)
        self.verify()
        temporary = 'pending-' + secrets.token_hex(16)
        fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                     0o600, dir_fd=self.fd)
        try:
            view = memoryview(data)
            while view:
                written = os.write(fd, view)
                require(written > 0)
                view = view[written:]
            os.fsync(fd)
            self.verify()
            if replace:
                os.rename(temporary, name, src_dir_fd=self.fd, dst_dir_fd=self.fd)
            else:
                os.link(temporary, name, src_dir_fd=self.fd, dst_dir_fd=self.fd, follow_symlinks=False)
                os.unlink(temporary, dir_fd=self.fd)
            os.fsync(self.fd)
            require(self.read(name) == data)
        finally:
            os.close(fd)
            # A failed publication is retained, not blindly cleaned up or retried.

    def usage(self):
        self.verify()
        total = 0
        count = 0
        with os.scandir(self.fd) as entries:
            for count, entry in enumerate(entries, 1):
                require(count <= MAX_RECORDS and digest(entry.name))
                slot = self.child(entry.name)
                try:
                    total += slot.record_info().st_size
                finally:
                    slot.close()
                require(total <= CAPACITY)
        self.verify()
        return CAPACITY if count == MAX_RECORDS else total

    def record_info(self):
        self.verify()
        with os.scandir(self.fd) as entries:
            first = next(entries, None)
            require(first is not None and first.name == 'record.hzov' and next(entries, None) is None)
            info = first.stat(follow_symlinks=False)
        require(stat.S_ISREG(info.st_mode) and info.st_uid == os.geteuid()
                and stat.S_IMODE(info.st_mode) == 0o600 and info.st_nlink == 1
                and 0 < info.st_size <= 160 * 1024 * 1024)
        self.verify()
        return info

    def record(self, manifest):
        require(digest(manifest))
        slot = self.child(manifest)
        try:
            before = slot.record_info()
            data = slot.read('record.hzov', 160 * 1024 * 1024)
            require(identity(before) == identity(slot.record_info()))
            self.verify()
            return data
        finally:
            slot.close()

    def verified_record(self, observation):
        require(digest(observation['manifest']) and digest(observation['record_sha256'])
                and type(observation['record_bytes']) is int
                and 0 < observation['record_bytes'] <= 160 * 1024 * 1024)
        data = self.record(observation['manifest'])
        require(data is not None and len(data) == observation['record_bytes']
                and hashlib.sha256(data).hexdigest() == observation['record_sha256'])


def open_slot(base, binding, create=False):
    require(digest(binding))
    worker = Directory(base)
    try:
        if create:
            try:
                os.mkdir('byte-captures', 0o700, dir_fd=worker.fd)
                os.fsync(worker.fd)
            except FileExistsError:
                pass
        captures = worker.child('byte-captures')
        try:
            return captures.child(binding, create)
        finally:
            captures.close()
    finally:
        worker.close()
