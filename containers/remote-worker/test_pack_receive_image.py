#!/usr/bin/env python3
"""Exercise explicit pack commands on disposable synthetic retained storage."""

import argparse
import json
from pathlib import Path
import struct

from test_overlay_receive_image import identity
from test_repository_image import ImageSmoke, digest, encoded, snapshot


def frame(request, payload):
    header = encoded(request)
    return struct.pack('<I', len(header)) + header + payload


def prepare(smoke, base, name):
    view = smoke.root / name
    view.mkdir(mode=0o700)
    smoke.git(view, 'init', '--bare', '--template=', '--object-format=sha1')
    (view / 'objects/info/alternates').write_text(str(smoke.root / 'source/objects') + '\n')
    (view / 'HEAD').write_text(base + '\n')
    (view / 'shallow').write_text(base + '\n')
    payload = smoke.git(view, 'pack-objects', '--stdout', '--revs', '--shallow',
                        '--no-reuse-delta', '--no-reuse-object', '--window=0', '--threads=1',
                        data=(base + '\n').encode()).stdout
    return payload, {'base_commit': base, 'sha256': digest(payload), 'encoded_bytes': len(payload)}


def state(root):
    return snapshot(root), {str(path.relative_to(root)): identity(path)
                            for path in [root, *root.rglob('*')]}


def test(smoke):
    smoke.fixture()
    before = snapshot(smoke.root / 'source'), snapshot(smoke.root / 'bundles')
    payload, pack = prepare(smoke, smoke.ancestor, 'small-view')

    def private(name, expected=pack):
        root = smoke.root / 'retained' / name
        root.mkdir(mode=0o700)
        return root, {'version': 1, 'parent': '/retained/' + name, 'destination': 'ready', 'pack': expected}

    def observation(request, path=None):
        return {'version': 1, 'path': path or request['parent'] + '/' + request['destination'],
                'pack': request['pack']}

    def response(output, status, code):
        assert output.returncode == code and not output.stderr, (output.returncode, output.stdout, output.stderr)
        assert output.stdout.endswith(b'\n') and len(output.stdout) <= 128 * 1024
        value = json.loads(output.stdout)
        assert set(value) == {'version', 'status', 'pack', 'retained', 'reason'}
        assert value['version'] == 1 and value['status'] == status, value
        assert 'private-input-marker' not in str(value)
        if status in ('acknowledged', 'observed'):
            assert value['retained'] is None and value['reason'] is None
        else:
            assert value['pack'] is None and value['reason']
        return value

    def invoke(command, data, status, code):
        result = response(smoke.command('/usr/local/bin/horizon-repository', [command], data,
                                        mount_inputs=False), status, code)
        smoke.retire_completed()
        return result

    root, request = private('receiver')
    original = state(root)
    invoke('pack-status', encoded(observation(request)), 'error', 1)
    for invalid in (b'private-input-marker', frame(dict(request, version=2), payload),
                    frame(dict(request, destination='../escape'), payload),
                    frame(dict(request, pack=dict(pack, sha256='private-input-marker')), payload)):
        invoke('receive-pack', invalid, 'rejected', 2)
    invoke('receive-pack', frame(dict(request, pack=dict(pack, encoded_bytes=2**64-1)), payload), 'error', 1)
    assert state(root) == original
    receipt = invoke('receive-pack', frame(request, payload), 'acknowledged', 0)
    assert receipt['pack']['identity'] == pack and receipt['pack']['objects'] == 3
    assert receipt['pack']['path'] == '/retained/receiver/ready'
    assert receipt['pack']['objects_directory'] == '/retained/receiver/ready/decoded/objects'
    ready = root / 'ready'
    assert (ready / 'decoded/objects/pack' / ('pack-' + payload[-20:].hex() + '.pack')).read_bytes() == payload
    saved = state(ready)
    observed = invoke('pack-status', encoded(observation(request)), 'observed', 0)
    assert observed['pack'] == receipt['pack'] and state(ready) == saved
    wrong = dict(observation(request), pack=dict(pack, sha256='0'*64))
    invoke('pack-status', encoded(wrong), 'error', 1)
    assert state(ready) == saved

    collision = invoke('receive-pack', frame(request, payload), 'unpublished', 1)
    assert collision['retained']['destination'] is None
    retained_path = Path(collision['retained']['source'])
    assert retained_path.parent == Path(request['parent']) and retained_path.name != 'ready'
    retained = root / retained_path.name
    retained_state = state(retained)
    invoke('pack-status', encoded(observation(request, str(retained_path))), 'observed', 0)
    assert state(retained) == retained_state and state(ready) == saved and len(list(root.iterdir())) == 2

    for name, contents, expected in (
        ('truncated', payload[:-1], pack), ('trailing', payload+b'extra', pack),
        ('mismatch', payload, dict(pack, sha256='0'*64)),
    ):
        failed, request = private(name, expected)
        outcome = invoke('receive-pack', frame(request, contents), 'receive_unconfirmed', 1)
        assert outcome['retained']['destination'] is None
        source = Path(outcome['retained']['source'])
        assert source.parent == Path(request['parent']) and source.name != 'ready'
        assert (failed / source.name).is_dir() and not (failed / 'ready').exists()
        saved_failure = state(failed)
        invoke('pack-status', encoded(observation(request)), 'error', 1)
        assert state(failed) == saved_failure

    sentinel, request = private('sentinel')
    (sentinel / 'ready').write_bytes(b'private-input-marker')
    saved_sentinel = identity(sentinel / 'ready')
    invoke('receive-pack', frame(request, payload), 'unpublished', 1)
    assert (sentinel / 'ready').read_bytes() == b'private-input-marker'
    assert identity(sentinel / 'ready') == saved_sentinel
    missing = dict(request, parent='/retained/missing')
    invoke('receive-pack', frame(missing, payload), 'error', 1)
    invoke('pack-status', encoded(observation(missing)), 'error', 1)
    assert not (smoke.root / 'retained/missing').exists()
    unsafe, request = private('unsafe')
    unsafe.chmod(0o755)
    saved_unsafe = state(unsafe)
    invoke('receive-pack', frame(request, payload), 'error', 1)
    assert state(unsafe) == saved_unsafe

    lost, request = private('lost-output')
    output = smoke.command('/bin/sh', ['-c', 'exec /usr/local/bin/horizon-repository receive-pack > /dev/full'],
                           frame(request, payload), mount_inputs=False)
    assert output.returncode == 3 and not output.stdout
    assert output.stderr == b'Could not write a complete response; retain data and inspect before any retry.\n'
    saved_lost = state(lost)
    smoke.retire_completed()
    invoke('pack-status', encoded(observation(request)), 'observed', 0)
    assert state(lost) == saved_lost and len(list(lost.iterdir())) == 1

    output = smoke.command('/usr/bin/python3', ['-c',
        "import json,os,subprocess,sys; os.mkdir('/tmp/private-smoke',0o700); "
        "r=subprocess.run(['/usr/local/bin/horizon-repository','receive-pack'],stdin=sys.stdin.buffer,capture_output=True); "
        "v=json.loads(r.stdout); assert v['status']=='unpublished'; "
        "assert os.path.isdir(v['retained']['source']); assert not os.path.exists('/tmp/private-smoke/ready'); "
        "sys.stdout.buffer.write(r.stdout); sys.stderr.buffer.write(r.stderr); sys.exit(r.returncode)"],
        frame(dict(request, parent='/tmp/private-smoke'), payload), overlay=True, mount_inputs=False)
    response(output, 'unpublished', 1)
    smoke.retire_completed()

    large_payload, large_pack = prepare(smoke, smoke.base, 'large-view')
    assert len(large_payload) > 65*1024*1024
    large, request = private('large', large_pack)
    receipt = invoke('receive-pack', frame(request, large_payload), 'acknowledged', 0)
    assert receipt['pack']['identity'] == large_pack and receipt['pack']['objects'] == 7
    saved_large = state(large)
    observed = invoke('pack-status', encoded(observation(request)), 'observed', 0)
    assert receipt['pack'] == observed['pack'] and state(large) == saved_large
    checkout, _ = private('checkout')
    materialize = dict(smoke.request, scratch_parent='/retained/checkout',
                       objects_directory=receipt['pack']['objects_directory'])
    smoke.materialize(encoded(materialize), 'published', 0)
    smoke.verify_checkout(checkout / 'published')
    assert state(large) == saved_large
    assert before == (snapshot(smoke.root / 'source'), snapshot(smoke.root / 'bundles'))
    assert not list((smoke.root / 'retained').rglob('setup-claim.json'))
    print('PASS pack commands: small and 65 MiB-plus exact-base pack receipt/publication, fresh-container '
          'non-mutating observation, retained failed transfers/collisions, no root creation, unsafe and '
          'unqualified storage rejection, output-loss recovery, and large shallow raw checkout with '
          'overlay/index/LFS/link/mode semantics; source inputs unchanged and no setup started. '
          'Synthetic local proof, not client transport or cloud/PC-off durability.', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--image', required=True)
    parser.add_argument('--docker-host', required=True)
    parser.add_argument('--fixture-parent', default='/tmp')
    smoke = ImageSmoke(parser.parse_args())
    print(f'Task-owned pack fixture: {smoke.root}; label: {smoke.label}; image: {smoke.image}', flush=True)
    try:
        test(smoke)
    finally:
        smoke.close()


if __name__ == '__main__':
    main()
