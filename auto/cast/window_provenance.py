"""Identity checks for the whole-window workload's independent executable tools."""
import hashlib
from pathlib import Path
import shutil
import subprocess


def executable_sha256(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def verify_file(path, expected_sha256):
    path = Path(path).resolve(strict=True)
    if executable_sha256(path) != expected_sha256:
        raise RuntimeError('frozen executable changed: ' + path.name)
    return str(path)


def resolved_tool(name):
    path = shutil.which(name)
    if not path:
        raise RuntimeError('missing independent decoder tool: ' + name)
    return str(Path(path).resolve(strict=True))


def decoder_identities():
    identities = {}
    for name in ('ffmpeg', 'ffprobe'):
        path = resolved_tool(name)
        digest = executable_sha256(path)
        version = subprocess.check_output([path, '-version'], text=True, timeout=10).splitlines()[0]
        verify_file(path, digest)
        identities[name] = {'path': path, 'sha256': digest, 'version': version}
    return identities


def verified_decoders(identities):
    paths = {}
    for name in ('ffmpeg', 'ffprobe'):
        expected = identities[name]
        if resolved_tool(name) != expected['path']:
            raise RuntimeError('independent decoder tool resolution changed: ' + name)
        paths[name] = verify_file(expected['path'], expected['sha256'])
    return paths
