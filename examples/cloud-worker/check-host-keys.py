#!/usr/bin/env python3
"""Fail when any layer of a worker image carries SSH host keys under /etc/ssh.

Every worker started from a published image would share such a key. Each worker
creates its own host keys when it starts, so an image needs none. A file removed by
a later layer is still in the earlier layer, so every layer is read, not only the
final file system. The image is read as a stream from `docker save`.
"""
import argparse
import json
import posixpath
import re
import subprocess
import sys
import tarfile


HOST_KEY = re.compile(r'(?:^|/)etc/ssh/ssh_host_[^/]*$')


class CheckedHeader(tarfile.TarInfo):
    """Marks a layer whose header is malformed or truncated. tarfile ends the member
    list there without an error, so the rest of such a layer would go unread."""

    @classmethod
    def fromtarfile(cls, tarfile_):
        try:
            return super().fromtarfile(tarfile_)
        except tarfile.EOFHeaderError:
            raise
        except tarfile.HeaderError:
            tarfile_.malformed = True
            raise


def layer_host_keys(layer):
    """The SSH host key paths in one layer, read as a stream from `layer`.

    Raises tarfile.ReadError when the layer does not end with the end-of-archive marker.
    """
    with tarfile.open(fileobj=layer, mode='r|*', tarinfo=CheckedHeader) as tar:
        keys = [member.name for member in tar if HOST_KEY.search(member.name)]
        if getattr(tar, 'malformed', False):
            raise tarfile.ReadError('The layer is malformed or truncated')
    return keys


def image_host_keys(stream):
    """The host keys in every layer of a `docker save` archive, by layer path.

    Raises ValueError when a layer that the archive's manifest names could not be read,
    so an unreadable layer never passes as a clean one.
    """
    found = {}
    scanned = set()
    links = {}
    listed = None
    with tarfile.open(fileobj=stream, mode='r|') as archive:
        for member in archive:
            # Docker before 25 saves a repeated layer as a link to the first copy.
            if member.issym():
                links[member.name] = posixpath.normpath(
                    posixpath.join(posixpath.dirname(member.name), member.linkname))
            elif member.islnk():
                links[member.name] = member.linkname
            if not member.isfile():
                continue
            content = archive.extractfile(member)
            if member.name == 'manifest.json':
                listed = {layer for image in json.load(content) for layer in image.get('Layers', [])}
                continue
            try:
                keys = layer_host_keys(content)
            except tarfile.TarError:
                continue
            scanned.add(member.name)
            if keys:
                found[member.name] = keys
    for _ in links:
        for name, target in links.items():
            if target in scanned:
                scanned.add(name)
                if target in found:
                    found[name] = found[target]
    if listed is None:
        raise ValueError('The image archive has no manifest.json')
    unread = sorted(listed - scanned)
    if unread:
        raise ValueError('Image layers could not be read: ' + ', '.join(unread))
    return found


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('image', help='Local image reference')
    parser.add_argument('--docker', default='docker')
    args = parser.parse_args()
    if not args.image or args.image.startswith('-') or any(character.isspace() for character in args.image):
        print('Invalid image reference', file=sys.stderr)
        return 1
    with subprocess.Popen([args.docker, 'save', args.image], stdout=subprocess.PIPE) as save:
        try:
            found = image_host_keys(save.stdout)
        except (ValueError, tarfile.TarError, json.JSONDecodeError) as error:
            save.kill()
            print(error, file=sys.stderr)
            return 1
    if save.returncode != 0:
        print('docker save failed', file=sys.stderr)
        return 1
    if found:
        for layer, keys in sorted(found.items()):
            print(f'{layer}: ' + ', '.join(keys), file=sys.stderr)
        print('The image contains SSH host keys under /etc/ssh. Remove them in the layer that creates them.',
              file=sys.stderr)
        return 1
    print('No image layer contains SSH host keys.')
    return 0


if __name__ == '__main__':
    sys.exit(main())
