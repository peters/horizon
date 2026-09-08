#!/usr/bin/env python3
"""Synthetic framed overlay receipt and fresh-process acknowledgement recovery."""

import argparse
import json
import stat
import struct

from test_repository_image import ImageSmoke, digest, encoded, snapshot


def frame(request, payload):
    header = encoded(dict(request, encoded_bytes=len(payload)))
    return struct.pack('<I', len(header))+header+payload


def identity(path):
    value = path.lstat()
    return value.st_dev, value.st_ino, value.st_nlink, value.st_mtime_ns, stat.S_IMODE(value.st_mode)


def large_bundle(smoke):
    payloads = [smoke.large[:33*1024*1024], smoke.large[33*1024*1024:]]
    changes = [{'path': name, 'kind': 'file', 'sha256': digest(payload), 'bytes': len(payload),
                'executable': False} for name, payload in zip(('transfer-a', 'transfer-b'), payloads)]
    metadata = encoded({'domain': 'horizon.repository-overlay', 'version': 1,
        'repository': 'synthetic/project', 'commit': smoke.base, 'branch': None,
        'index': [], 'working_tree': changes})
    result = bytearray(b'HZOVLY\0\x01'+struct.pack('<I', len(metadata))+metadata+struct.pack('<I', 2))
    for key, payload in sorted((digest(payload), payload) for payload in payloads):
        result.extend(key.encode()+struct.pack('<Q', len(payload)))
        result.extend(payload)
    return digest(metadata), bytes(result)


def test(smoke):
    smoke.fixture()
    before = snapshot(smoke.root / 'source'), snapshot(smoke.root / 'bundles')
    payload = (smoke.root / 'bundles' / (smoke.manifest+'.hzov')).read_bytes()

    def private(name):
        root = smoke.root / 'retained' / name
        root.mkdir(mode=0o700)
        return root, {'version': 1, 'bundle_store': '/retained/'+name, 'bundle_manifest': smoke.manifest}

    def invoke(command, data, state, code):
        output = smoke.command('/usr/local/bin/horizon-repository', [command], data, mount_inputs=False)
        assert output.returncode == code and not output.stderr, (output.returncode, output.stderr)
        assert output.stdout.endswith(b'\n') and len(output.stdout) <= 1024
        response = json.loads(output.stdout)
        assert response['version'] == 1 and response['status'] == state, response
        assert set(response) == {'version', 'status', 'bundle_manifest', 'reason'}
        assert 'private-input-marker' not in str(response)
        smoke.retire_completed()
        return response

    root, request = private('receiver')
    invoke('overlay-status', encoded(request), 'missing', 4)
    assert not list(root.iterdir())
    for malformed in (frame(request, payload)[:-1], frame(request, payload)+b'extra',
                      frame(dict(request, bundle_manifest='a'*64), payload), b'private-input-marker'):
        invoke('receive-overlay', malformed, 'rejected', 2)
        assert not list(root.iterdir())
    receipt = invoke('receive-overlay', frame(request, payload), 'acknowledged', 0)
    assert receipt['bundle_manifest'] == smoke.manifest and receipt['reason'] is None
    record = root / (smoke.manifest+'.hzov')
    original = identity(record)
    assert original[2] == 1 and original[4] == 0o600 and record.read_bytes() == payload
    observed = invoke('overlay-status', encoded(request), 'observed', 0)
    assert observed['bundle_manifest'] == smoke.manifest and identity(record) == original
    invoke('receive-overlay', frame(request, payload), 'acknowledged', 0)
    assert identity(record) == original and record.read_bytes() == payload and len(list(root.iterdir())) == 1

    for name in ('conflict', 'hardlink', 'symlink'):
        unsafe, expected = private(name)
        destination = unsafe / (smoke.manifest+'.hzov')
        if name == 'symlink':
            destination.symlink_to(record)
        elif name == 'hardlink':
            other = unsafe / 'other'
            other.write_bytes(payload)
            other.chmod(0o600)
            destination.hardlink_to(other)
        else:
            destination.write_bytes(b'private-input-marker')
            destination.chmod(0o600)
        retained = identity(destination), snapshot(unsafe)
        invoke('receive-overlay', frame(expected, payload), 'write_unconfirmed', 1)
        invoke('overlay-status', encoded(expected), 'error', 1)
        assert retained == (identity(destination), snapshot(unsafe))
        assert identity(record) == original and record.read_bytes() == payload

    missing = dict(request, bundle_store='/retained/missing')
    invoke('overlay-status', encoded(missing), 'error', 1)
    invoke('receive-overlay', frame(missing, payload), 'error', 1)
    assert not (smoke.root / 'retained/missing').exists()
    lost, expected = private('lost-output')
    output = smoke.command('/bin/sh', ['-c', 'exec /usr/local/bin/horizon-repository receive-overlay > /dev/full'],
                           frame(expected, payload), mount_inputs=False)
    assert output.returncode == 3 and not output.stdout
    assert output.stderr == b'Could not write a complete response; retain data and inspect before any retry.\n'
    lost_record = lost / (smoke.manifest+'.hzov')
    saved = identity(lost_record)
    assert lost_record.read_bytes() == payload
    smoke.retire_completed()
    invoke('overlay-status', encoded(expected), 'observed', 0)
    assert identity(lost_record) == saved

    large, expected = private('large')
    manifest, contents = large_bundle(smoke)
    assert len(contents) > 65*1024*1024
    expected['bundle_manifest'] = manifest
    invoke('receive-overlay', frame(expected, contents), 'acknowledged', 0)
    large_record = large / (manifest+'.hzov')
    saved = identity(large_record)
    assert large_record.read_bytes() == contents
    invoke('overlay-status', encoded(expected), 'observed', 0)
    invoke('receive-overlay', frame(expected, contents), 'acknowledged', 0)
    assert identity(large_record) == saved and large_record.read_bytes() == contents
    assert before == (snapshot(smoke.root / 'source'), snapshot(smoke.root / 'bundles'))
    assert not list((smoke.root / 'retained').rglob('setup-claim.json'))
    print('PASS overlay receiver: exact small and 65 MiB-plus multi-blob records, fresh-process '
          'observation, immutable retries, strict malformed input, no root creation, unsafe/conflicting '
          'records retained and output-loss recovery; source inputs unchanged and no setup started. '
          'Synthetic local storage proof, not client export or cloud/checkpoint durability.', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--image', required=True)
    parser.add_argument('--docker-host', required=True)
    parser.add_argument('--fixture-parent', default='/tmp')
    smoke = ImageSmoke(parser.parse_args())
    print(f'Task-owned overlay fixture: {smoke.root}; label: {smoke.label}; image: {smoke.image}', flush=True)
    try:
        test(smoke)
    finally:
        smoke.close()


if __name__ == '__main__':
    main()
