#!/usr/bin/env python3
"""Pure guards by default; --binary PATH explicitly enables private bwrap CLI proof."""

import hashlib
import importlib.util
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('capture', HERE / 'byte-capture.py')
capture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(capture)
store = capture.store
BINDING = 'a' * 64
BINARY = None


class PrivateFixture(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='horizon-byte-capture-test-')
        self.root = Path(self.temporary.name)
        self.base = self.root / 'worker'
        self.base.mkdir(mode=0o700)
        self.slot = store.open_slot(str(self.base), BINDING, True)
        self.slot.write('enrollment.json', {'fixture': True})
        for name in ('bundles', 'receipts'):
            self.slot.child(name, True).close()
        self.base_patch = patch.object(capture, 'BASE', str(self.base))
        self.base_patch.start()

    def tearDown(self):
        self.base_patch.stop()
        self.slot.close()
        self.temporary.cleanup()

    def observation(self, data):
        digest = hashlib.sha256(data).hexdigest()
        directory = Path(self.slot.path) / 'bundles' / digest
        directory.mkdir(mode=0o700, exist_ok=True)
        path = directory / 'record.hzov'
        if not path.exists():
            path.write_bytes(data)
            path.chmod(0o600)
        return {'version': 1, 'binding': BINDING, 'manifest': digest,
                'record_sha256': digest, 'record_bytes': len(data), 'reason': None}

    def run_service(self, observations):
        with patch.object(capture, 'INTERVAL', 0), patch.object(capture, 'invoke', side_effect=observations):
            capture.service(BINDING)
        return store.decode(self.slot.read('status.json'))

    def test_previous_versions_survive_failed_next_attempt_and_no_relaunch(self):
        first, second = self.observation(b'first bytes'), self.observation(b'second bytes')
        state = self.run_service([first, second, store.CaptureError('failure')])
        self.assertEqual((state['state'], state['generation']), ('error', 2))
        self.assertEqual(state['last_success']['manifest'], second['manifest'])
        self.assertEqual(len(list((Path(self.slot.path) / 'bundles').iterdir())), 2)
        self.assertEqual(len(list((Path(self.slot.path) / 'receipts').iterdir())), 2)
        with patch.object(capture, 'invoke', return_value={'binding': BINDING}), patch.object(capture, 'status', return_value={'existing': True}), patch.object(capture.subprocess, 'Popen') as spawn:
            self.assertEqual(capture.start({'fixture': True}), {'existing': True})
            spawn.assert_not_called()

    def test_readback_or_receipt_failure_cannot_advance_success(self):
        first, second = self.observation(b'first'), self.observation(b'second')
        write = store.Directory.write
        def refuse_receipt(directory, name, value, replace=False):
            if name == second['manifest'] + '.json':
                raise OSError('synthetic receipt sync failure')
            return write(directory, name, value, replace)
        with patch.object(store.Directory, 'write', new=refuse_receipt):
            state = self.run_service([first, second])
        self.assertEqual(state['generation'], 1)
        self.assertEqual(state['last_success']['manifest'], first['manifest'])
        self.assertEqual(state['state'], 'error')

    def test_changed_is_retried_without_advancing_success_and_recovery_clears_gap(self):
        first, second = self.observation(b'first'), self.observation(b'second')
        observed = []
        results = iter([store.CaptureError('changed'), first, store.CaptureError('changed'), second, store.CaptureError('identity')])
        def invoke(*_):
            observed.append(store.decode(self.slot.read('status.json')))
            result = next(results)
            if isinstance(result, Exception):
                raise result
            return result
        with patch.object(capture, 'INTERVAL', 0), patch.object(capture, 'invoke', side_effect=invoke):
            capture.service(BINDING)
        self.assertEqual([value['state'] for value in observed], ['running', 'degraded', 'running', 'degraded', 'running'])
        self.assertEqual([value['generation'] for value in observed], [0, 0, 1, 1, 2])
        self.assertIsNone(observed[1]['last_success'])
        self.assertEqual(observed[3]['last_success'], observed[2]['last_success'])
        self.assertIsNone(observed[4]['last_error'])
        state = store.decode(self.slot.read('status.json'))
        self.assertEqual((state['state'], state['last_error'], state['generation']), ('error', 'identity', 2))

    def test_identical_record_at_byte_or_count_capacity_is_still_verified(self):
        value = self.observation(b'full')
        for capacity, records in [(4, 4096), (100, 1)]:
            with self.subTest(capacity=capacity), patch.object(store, 'CAPACITY', capacity), patch.object(store, 'MAX_RECORDS', records):
                with patch.object(capture, 'INTERVAL', 0), patch.object(capture, 'invoke', side_effect=[value, store.CaptureError('capacity')]) as invoke:
                    capture.service(BINDING)
                self.assertEqual(invoke.call_args_list[0].args[2], 0)
                state = store.decode(self.slot.read('status.json'))
                self.assertEqual((state['generation'], state['last_error']), (1, 'capacity'))
                self.assertEqual(state['last_success']['manifest'], value['manifest'])
            # Separate synthetic service enrollment, not a product restart path.
            (Path(self.slot.path) / 'lock').unlink()

    def test_lost_publication_reply_retains_orphan_without_success(self):
        value = self.observation(b'published before reply was lost')
        state = self.run_service([store.CaptureError('storage')])
        self.assertIsNone(state['last_success'])
        self.assertEqual(state['generation'], 0)
        self.assertEqual(list((Path(self.slot.path) / 'receipts').iterdir()), [])
        bundles = self.slot.child('bundles')
        try:
            bundles.verified_record(value)
            self.assertEqual(bundles.usage(), value['record_bytes'])
        finally:
            bundles.close()

    def test_deep_json_refuses_without_disclosing_input_or_traceback(self):
        raw = b'[' * 60000 + b'"private sentinel"' + b']' * 60000
        with self.assertRaisesRegex(store.CaptureError, '^invalid capture JSON$'):
            store.decode(raw)
        result = subprocess.run([sys.executable, '-I', '-B', str(HERE / 'byte-capture.py'), 'start'], input=raw, capture_output=True, timeout=5)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, b'')
        self.assertEqual(result.stderr, b'Byte capture unavailable; retain all data and inspect status.\n')

    def test_corrupt_published_bytes_fail_without_a_watermark(self):
        value = self.observation(b'original')
        (Path(self.slot.path) / 'bundles' / value['manifest'] / 'record.hzov').write_bytes(b'corrupt')
        state = self.run_service([value])
        self.assertEqual(state['generation'], 0)
        self.assertIsNone(state['last_success'])

    def test_named_accounting_rejects_partial_foreign_and_linked_slots(self):
        good = self.observation(b'prior verified bytes')
        for index, kind in enumerate(('empty', 'pending', 'extra', 'symlink', 'flat')):
            container = self.slot.child('case-' + str(index), True)
            try:
                root = Path(container.path)
                digest = 'b' * 64
                if kind == 'flat':
                    (root / (digest + '.hzov')).write_bytes(b'foreign anonymous layout')
                else:
                    directory = root / digest
                    directory.mkdir(mode=0o700)
                    if kind == 'symlink':
                        os.symlink(Path(self.slot.path) / 'bundles' / good['manifest'] / 'record.hzov', directory / 'record.hzov')
                    elif kind != 'empty':
                        name = 'pending' if kind == 'pending' else 'record.hzov'
                        (directory / name).write_bytes(b'partial')
                        (directory / name).chmod(0o600)
                        if kind == 'extra':
                            (directory / 'pending').write_bytes(b'never replay')
                before = sorted(str(path.relative_to(root)) for path in root.rglob('*'))
                with self.assertRaises((store.CaptureError, OSError)):
                    container.usage()
                with self.assertRaises((store.CaptureError, OSError)):
                    container.record(digest)
                self.assertEqual(sorted(str(path.relative_to(root)) for path in root.rglob('*')), before)
            finally:
                container.close()
        bundles = self.slot.child('bundles')
        try:
            bundles.verified_record(good)
        finally:
            bundles.close()

    def test_cancel_before_capture_never_invokes_child(self):
        self.slot.write('cancel.json', {'cancel': True})
        with patch.object(capture, 'invoke') as invoke:
            capture.service(BINDING)
            invoke.assert_not_called()
        self.assertEqual(store.decode(self.slot.read('status.json'))['state'], 'cancelled')

    def test_cancel_during_readback_preserves_prior_success(self):
        value = self.observation(b'bytes')
        def invoke(*_):
            self.slot.write('cancel.json', {'cancel': True})
            return value
        with patch.object(capture, 'invoke', side_effect=invoke):
            capture.service(BINDING)
        state = store.decode(self.slot.read('status.json'))
        self.assertEqual(state['state'], 'cancelled')
        self.assertIsNone(state['last_success'])

    def test_access_time_is_ignored_but_all_integrity_fields_are_compared(self):
        self.slot.write('value.json', {'safe': True})
        path = Path(self.slot.path) / 'value.json'
        before = path.stat()
        os.utime(path, ns=(before.st_atime_ns + 1_000_000, before.st_mtime_ns))
        self.assertEqual(store.decode(self.slot.read('value.json')), {'safe': True})
        fields = ('st_dev', 'st_ino', 'st_mode', 'st_uid', 'st_gid', 'st_nlink', 'st_size', 'st_mtime_ns', 'st_ctime_ns')
        values = dict((field, getattr(before, field)) for field in fields)
        from types import SimpleNamespace
        for field in fields:
            changed = dict(values, **{field: values[field] + 1})
            self.assertNotEqual(store.identity(SimpleNamespace(**values)), store.identity(SimpleNamespace(**changed)))

    def test_private_read_rejects_links_fifo_wrong_mode_and_capacity(self):
        self.slot.write('original.json', {'safe': True})
        root = Path(self.slot.path)
        os.symlink(root / 'original.json', root / 'link.json')
        os.link(root / 'original.json', root / 'hard.json')
        os.mkfifo(root / 'fifo.json', 0o600)
        for name in ('link.json', 'original.json', 'hard.json', 'fifo.json'):
            with self.assertRaises((OSError, store.CaptureError)):
                self.slot.read(name)
        self.slot.write('mode.json', {})
        (root / 'mode.json').chmod(0o644)
        with self.assertRaises(store.CaptureError):
            self.slot.read('mode.json')
        self.observation(b'12345')
        bundles = self.slot.child('bundles')
        try:
            with patch.object(store, 'CAPACITY', 4), self.assertRaises(store.CaptureError):
                bundles.usage()
        finally:
            bundles.close()

    def test_ancestry_swap_and_unknown_record_names_fail_closed(self):
        root = Path(self.slot.path)
        renamed = root.with_name('old')
        root.rename(renamed)
        root.mkdir(mode=0o700)
        with self.assertRaises(store.CaptureError):
            self.slot.read('enrollment.json')
        root.rmdir()
        renamed.rename(root)
        bundles = self.slot.child('bundles')
        try:
            (Path(bundles.path) / 'unaccounted').write_bytes(b'x')
            with self.assertRaises(store.CaptureError):
                bundles.usage()
        finally:
            bundles.close()

    def test_status_is_noncreating_and_stale_or_corrupt_never_looks_current(self):
        self.assertEqual(capture.status('b' * 64)['state'], 'absent')
        self.assertFalse((Path(self.slot.path).parent / ('b' * 64)).exists())
        value = self.observation(b'content')
        state = self.run_service([value, store.CaptureError('failure')])
        state['interval_seconds'] = capture.INTERVAL
        for recorded in ('running', 'degraded'):
            state['state'] = recorded
            self.slot.write('status.json', state, True)
            with patch.object(capture, 'now', return_value=state['last_success']['verified_at_millis'] + 31_000):
                self.assertEqual(capture.status(BINDING)['state'], 'stale')
        state['private-extra'] = 'must not be emitted'
        self.slot.write('status.json', state, True)
        with self.assertRaises(store.CaptureError):
            capture.status(BINDING)

    def test_actual_child_exit_race_false_green_output_and_deadline(self):
        helper = self.root / 'helper'
        response = {'version': 1, 'binding': BINDING, 'manifest': None,
                    'record_sha256': None, 'record_bytes': None, 'reason': None}
        for tail, accepted in [('', True), ('sys.exit(1)', False),
                               ('print("x"*2000)', False), ('time.sleep(5)', False)]:
            helper.write_text('#!/usr/bin/python3\nimport sys,time\nsys.stdin.buffer.read()\nprint(' + repr(json.dumps(response)) + ')\n' + tail + '\n')
            helper.chmod(0o700)
            with patch.object(capture, 'HELPER', str(helper)), patch.object(capture, 'ATTEMPT_SECONDS', .3):
                if accepted:
                    self.assertEqual(capture.invoke({}, True, 0), response)
                else:
                    with self.assertRaises((store.CaptureError, subprocess.SubprocessError)):
                        capture.invoke({}, True, 0)

    def test_changed_binding_and_capacity_exhaustion_retain_success(self):
        first = self.observation(b'first')
        wrong = dict(self.observation(b'wrong'), binding='b' * 64)
        state = self.run_service([first, wrong])
        self.assertEqual(state['generation'], 1)
        self.assertEqual(state['last_success']['manifest'], first['manifest'])
        self.assertEqual(state['state'], 'error')


def literal_bundle(raw, manifest):
    """Test-only v1 decoding: recover exact selected payloads, never apply to checkout."""
    store.require(raw[:8] == b'HZOVLY\x00\x01')
    length = int.from_bytes(raw[8:12], 'little')
    metadata = raw[12:12 + length]
    store.require(hashlib.sha256(metadata).hexdigest() == manifest)
    plan = store.decode(metadata)
    offset = 12 + length
    count = int.from_bytes(raw[offset:offset + 4], 'little')
    offset += 4
    blobs = {}
    for _ in range(count):
        digest = raw[offset:offset + 64].decode()
        size = int.from_bytes(raw[offset + 64:offset + 72], 'little')
        offset += 72
        payload = raw[offset:offset + size]
        offset += size
        store.require(len(payload) == size and hashlib.sha256(payload).hexdigest() == digest and digest not in blobs)
        blobs[digest] = payload
    store.require(offset == len(raw))
    return plan, blobs


def inside_proof():
    """Synthetic initial Git receipts only; actual capture/start/status code is unmodified."""
    root = Path('/workspace')
    for directory in ('horizon', '.horizon-worker', 'horizon/repository', '.horizon-worker/git-workspace'):
        (root / directory).mkdir(mode=0o700)
    checkout = root / 'horizon/repository'
    environment = dict(capture.ENV, GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_GLOBAL='/dev/null',
                       GIT_AUTHOR_NAME='Synthetic Test', GIT_AUTHOR_EMAIL='fixture@example.invalid',
                       GIT_COMMITTER_NAME='Synthetic Test', GIT_COMMITTER_EMAIL='fixture@example.invalid')
    def git(*arguments):
        return subprocess.run(['/usr/bin/git', '-C', str(checkout), *arguments], env=environment,
                              capture_output=True, check=True, timeout=10).stdout.strip().decode()
    git('init', '-b', 'work/capture')
    (checkout / 'selected.txt').write_bytes(b'base\n')
    (checkout / 'deleted.txt').write_bytes(b'deleted base\n')
    git('add', 'selected.txt', 'deleted.txt')
    git('commit', '-m', 'synthetic base')
    preparation = {'version': 1, 'workspace_local_id': 'fixture',
        'runtime_id': '00000000-0000-4000-8000-000000000001',
        'source': {'repository': 'fixture/repository', 'commit': git('rev-parse', 'HEAD'), 'branch': 'work/capture'},
        'work_branch': 'work/capture'}
    metadata = checkout.stat()
    claim = root / '.horizon-worker/git-workspace'
    for name, value in [('claim.json', preparation), ('complete.json',
            {'request': preparation, 'device': metadata.st_dev, 'inode': metadata.st_ino})]:
        (claim / name).write_bytes(json.dumps(value, separators=(',', ':')).encode())
        (claim / name).chmod(0o600)
    (checkout / 'selected.txt').write_bytes(b'staged bytes\n')
    git('add', 'selected.txt')
    (checkout / 'selected.txt').write_bytes(b'first working bytes\n')
    (checkout / 'untracked.txt').write_bytes(b'untracked protected bytes\n')
    (checkout / 'deleted.txt').unlink()
    (checkout / '.env').write_bytes(b'synthetic not selected\n')
    enrollment = {'version': 1, 'preparation': preparation,
                  'selected': ['selected.txt', 'untracked.txt', 'deleted.txt'], 'retained_volume_attested': True}
    command = ['/usr/bin/python3', '-I', '-B', '/usr/local/bin/horizon-byte-capture']
    def call(*args, data=None):
        result = subprocess.run(command + list(args), input=data, capture_output=True, env=capture.ENV, timeout=20)
        store.require(result.returncode == 0 and not result.stderr)
        return store.decode(result.stdout)
    result = call('start', data=store.encode(enrollment))
    binding = result['binding']
    try:
        def wait_generation(generation):
            deadline = time.monotonic() + 35
            while time.monotonic() < deadline:
                value = call('status', binding)
                if value.get('generation', 0) >= generation:
                    return value
                store.require(value['state'] not in ('error', 'cancelled'))
                time.sleep(.1)
            raise store.CaptureError('synthetic proof deadline')
        first = wait_generation(1)
        # The start/controller subprocess has exited; no handle or heartbeat remains.
        (checkout / 'selected.txt').write_bytes(b'second working bytes\n')
        second = wait_generation(first['generation'] + 1)
        store.require(first['last_success']['manifest'] != second['last_success']['manifest'])
        interval = second['last_success']['verified_at_millis'] - first['last_success']['verified_at_millis']
        store.require(0 < interval <= 30_000)
        slot = store.open_slot(capture.BASE, binding)
        bundles = slot.child('bundles')
        try:
            for record, payload in [(first, b'first working bytes\n'), (second, b'second working bytes\n')]:
                success = record['last_success']
                bundles.verified_record(success)
                raw = bundles.record(success['manifest'])
                plan, blobs = literal_bundle(raw, success['manifest'])
                index = {entry['path']: entry for entry in plan['index']}
                working = {entry['path']: entry for entry in plan['working_tree']}
                store.require(blobs[index['selected.txt']['sha256']] == b'staged bytes\n'
                    and blobs[working['selected.txt']['sha256']] == payload
                    and blobs[working['untracked.txt']['sha256']] == b'untracked protected bytes\n'
                    and working['deleted.txt']['kind'] == 'remove' and b'synthetic not selected' not in raw)
                recovered = root / ('recovered-' + str(record['generation']))
                recovered.mkdir(mode=0o700)
                (recovered / 'selected.txt').write_bytes(blobs[working['selected.txt']['sha256']])
                store.require((recovered / 'selected.txt').read_bytes() == payload)
        finally:
            bundles.close()
            slot.close()
        repeat = call('start', data=store.encode(enrollment))
        store.require(repeat['binding'] == binding and repeat['state'] != 'submitted')
        git('commit', '-m', 'synthetic changed baseline')
        deadline = time.monotonic() + 25
        while True:
            refused = call('status', binding)
            if refused['state'] == 'error':
                break
            store.require(time.monotonic() < deadline)
            time.sleep(.1)
        store.require(refused['generation'] == second['generation']
                      and refused['last_success'] == second['last_success'])
        print(json.dumps({'proof': 'controller exited; two verified byte versions', 'observed_interval_millis': interval}))
    finally:
        call('cancel', binding)
        deadline = time.monotonic() + 20
        while call('status', binding).get('recorded_state') not in ('cancelled', 'error'):
            store.require(time.monotonic() < deadline)
            time.sleep(.1)


class ActualCli(unittest.TestCase):
    def test_explicit_local_namespace_controller_exit(self):
        if BINARY is None:
            self.skipTest('requires explicit --binary candidate after source/build review')
        binary = Path(BINARY).resolve(strict=True)
        info = binary.stat()
        self.assertTrue(stat.S_ISREG(info.st_mode) and info.st_uid == os.geteuid() and not info.st_mode & 0o022)
        with tempfile.TemporaryDirectory(prefix='horizon-byte-capture-native-') as temporary:
            arguments = ['/usr/bin/bwrap', '--unshare-all', '--new-session', '--die-with-parent', '--tmpfs', '/',
                '--ro-bind', '/usr', '/usr', '--symlink', 'usr/bin', '/bin', '--symlink', 'usr/lib', '/lib',
                '--symlink', 'usr/lib64', '/lib64', '--symlink', 'usr/sbin', '/sbin',
                '--dev', '/dev', '--proc', '/proc', '--tmpfs', '/tmp', '--tmpfs', '/home', '--tmpfs', '/root',
                '--tmpfs', '/run', '--tmpfs', '/etc', '--tmpfs', '/usr/local/bin',
                '--bind', temporary, '/workspace', '--ro-bind', str(binary), '/usr/local/bin/horizon-repository',
                '--ro-bind', str(HERE / 'byte-capture.py'), '/usr/local/bin/horizon-byte-capture',
                '--ro-bind', str(HERE / 'byte_capture_store.py'), '/usr/local/bin/byte_capture_store.py',
                '--ro-bind', str(HERE), '/proof', '/usr/bin/python3', '-I', '-B', '/proof/test_byte_capture.py', '--inside']
            result = subprocess.run(arguments, capture_output=True, env=capture.ENV, timeout=90, umask=0o077)
            self.assertEqual(result.returncode, 0, result.stderr.decode(errors='replace'))
            self.assertIn(b'two verified byte versions', result.stdout)


if __name__ == '__main__':
    if sys.argv[1:] == ['--inside']:
        inside_proof()
    else:
        if len(sys.argv) >= 3 and sys.argv[1] == '--binary':
            BINARY = sys.argv[2]
            del sys.argv[1:3]
        unittest.main()
