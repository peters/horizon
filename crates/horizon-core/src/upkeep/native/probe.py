"""Reads what the agent on a test cloud wrote to /workspace/home/native-test, for Horizon.

It runs as the account that owns /workspace/home: the agent account where the worker isolates
agents, root where agents run as root. Every file in the folder is untrusted: only regular
files with plain names are read, without following a link and without waiting on a pipe, and
each read has a limit.
"""
import base64
import json
import os
import re
import stat
import sys

FOLDER = '/workspace/home/native-test'
NAME = re.compile(r'[A-Za-z0-9][A-Za-z0-9._-]{0,79}')
# The kinds of evidence Horizon keeps; other files are not listed.
KINDS = ('png', 'jpg', 'jpeg', 'gif', 'webp', 'mp4', 'webm', 'md', 'txt', 'json')
FILES = 12
TEXTS = ('result.json', 'report.md')
FILE_LIMIT = 2 * 1024 * 1024
TEXT_LIMIT = 16 * 1024


def folder():
    try:
        return os.open(FOLDER, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    except OSError:
        return None


def regular(parent, name):
    """The open regular file `name` and its size, or None."""
    try:
        fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=parent)
    except OSError:
        return None
    info = os.fstat(fd)
    if not stat.S_ISREG(info.st_mode):
        os.close(fd)
        return None
    return fd, info.st_size


def text(parent, name):
    opened = regular(parent, name)
    if opened is None:
        return None
    fd, _ = opened
    with os.fdopen(fd, 'rb') as handle:
        return handle.read(TEXT_LIMIT).decode('utf-8', 'replace')


def status():
    parent = folder()
    if parent is None:
        return {'result': None, 'report': None, 'files': []}
    try:
        files = []
        for name in sorted(os.listdir(parent)):
            if not NAME.fullmatch(name) or name.rsplit('.', 1)[-1].lower() not in KINDS:
                continue
            # The result and the report do not take the place of evidence.
            if name not in TEXTS and sum(entry['name'] not in TEXTS for entry in files) >= FILES:
                continue
            opened = regular(parent, name)
            if opened is not None:
                os.close(opened[0])
                # A file over the limit is not kept, so it takes no place.
                if opened[1] <= FILE_LIMIT:
                    files.append({'name': name, 'size': opened[1]})
        return {'result': text(parent, 'result.json'), 'report': text(parent, 'report.md'), 'files': files}
    finally:
        os.close(parent)


def read_file(name):
    if not NAME.fullmatch(name):
        return None
    parent = folder()
    if parent is None:
        return None
    try:
        opened = regular(parent, name)
        if opened is None or opened[1] > FILE_LIMIT:
            if opened is not None:
                os.close(opened[0])
            return None
        with os.fdopen(opened[0], 'rb') as handle:
            data = handle.read(FILE_LIMIT + 1)
        return None if len(data) > FILE_LIMIT else base64.b64encode(data).decode()
    finally:
        os.close(parent)


def main():
    if os.stat('/workspace/home').st_uid != os.geteuid():
        sys.exit('The test cloud runs this check as another account than the owner of its home.')
    operation = sys.argv[1] if len(sys.argv) > 1 else 'status'
    if operation == 'status':
        print(json.dumps(status()))
    elif operation == 'file' and len(sys.argv) > 2:
        data = read_file(sys.argv[2])
        if data is None:
            sys.exit('The file is missing, too large or not a regular file.')
        print(data)
    else:
        sys.exit('Unknown operation.')


if __name__ == '__main__':
    main()
