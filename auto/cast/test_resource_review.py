"""Proposed regressions: actual resource bounds and delayed observation rejection."""
import json
from pathlib import Path
import tempfile
import threading
from types import SimpleNamespace
import unittest
from unittest.mock import MagicMock, patch
import whole_window
import window_resources as resources


def row(ticks):
    return {1: {'self_ticks': ticks, 'reaped_ticks': 0, 'rss_kib': 100}}


def sampler():
    value = object.__new__(resources.Sampler)
    value.thread = MagicMock(); value.thread.is_alive.return_value = False
    value.done = threading.Event(); value.begin = row(10); value.end = row(20)
    value.started, value.ended = 1, 2
    value.begin_snapshot_started, value.end_snapshot_started = .8, 1.8
    value.identity = {}; value.errors = []; value.gpu_errors = []; value.gpu = True
    value.readings = []
    return value


class ResourceIntervalReviewTests(unittest.TestCase):
    def test_snapshot_latency_is_recorded_and_frozen_before_join(self):
        value = sampler(); value.thread = None; value.end = None
        now = [1.0]
        def snapshot():
            now[0] += .25
            return row(10 if value.begin is None else 20)
        value.begin = None; value.snapshot = snapshot
        thread = MagicMock(); thread.is_alive.return_value = False
        thread.join.side_effect = lambda **kw: now.__setitem__(0, now[0] + 5)
        with patch.object(resources.time, 'monotonic', side_effect=lambda: now[0]), \
             patch.object(resources.threading, 'Thread', return_value=thread):
            value.start(); now[0] += 10; value.stop()
            value.readings = [{'elapsed_seconds': .2, 'tree_rss_kib': 100,
                               'gpu_memory_mib': None}]
            result = value.finish()
        self.assertEqual((result['began_monotonic'], result['ended_monotonic']), (1.25, 11.5))
        self.assertEqual(result['seconds'], 10.25)
        self.assertEqual(result['snapshot_envelopes'], {'begin': [1, 1.25], 'end': [11.25, 11.5]})

    def test_late_rss_and_gpu_queries_cannot_change_measured_peak(self):
        value = sampler()
        value.readings = [
            {'elapsed_seconds': -.1, 'tree_rss_kib': 900, 'gpu_memory_mib': None},
            {'elapsed_seconds': .2, 'tree_rss_kib': 100, 'gpu_memory_mib': 40,
             'gpu_observed_seconds': .3},
            {'elapsed_seconds': .9, 'tree_rss_kib': 120, 'gpu_memory_mib': 999,
             'gpu_observed_seconds': 3},
            {'elapsed_seconds': 2, 'tree_rss_kib': 9999, 'gpu_memory_mib': 9999,
             'gpu_observed_seconds': 3}]
        value.errors = [{'elapsed_seconds': 3, 'error': 'late snapshot exited'}]
        value.gpu_errors = [{'elapsed_seconds': 3, 'error': 'late driver failed'}]
        result = value.finish()
        self.assertEqual((result['peak_tree_rss_kib'], result['gpu_memory_mib']), (120, 40))
        self.assertEqual(result['excluded_rss_observations'], 2)
        self.assertEqual(result['excluded_late_gpu_observations'], 1)
        self.assertEqual(result['gpu_errors'], [])
        self.assertIsNone(result['readings'][1]['gpu_memory_mib'])
        self.assertEqual(value.readings[2]['gpu_memory_mib'], 999)

    def test_in_window_errors_still_fail_or_mark_gpu_unknown(self):
        value = sampler()
        value.readings = [{'elapsed_seconds': .2, 'tree_rss_kib': 100,
                           'gpu_memory_mib': 40, 'gpu_observed_seconds': .3}]
        value.gpu_errors = [{'elapsed_seconds': .4, 'error': 'driver unavailable'}]
        result = value.finish()
        self.assertIsNone(result['gpu_memory_mib'])
        self.assertEqual(result['gpu_errors'], ['driver unavailable'])
        value.errors = [{'elapsed_seconds': .4, 'error': 'snapshot unavailable'}]
        with self.assertRaisesRegex(RuntimeError, 'snapshot unavailable'):
            value.finish()

    def test_content_window_uses_sampler_endpoints_not_status_poll_times(self):
        class Captured(Exception):
            pass
        now, captured, starts = [0.0], {}, [0]
        class FakeSampler:
            def __init__(self, pid, gpu):
                pass
            def start(self):
                now[0] += .25; self.started = now[0]
            def stop(self):
                now[0] += .5; self.ended = now[0]
            def finish(self):
                return {'began_monotonic': self.started, 'ended_monotonic': self.ended,
                        'seconds': self.ended - self.started}
        def status(root):
            now[0] += 4
            return {'encoder': 'libx264', 'scaler': None, 'state': 'streaming',
                    'frames': int(now[0] * 10)}
        def request(root, role, operation, **kw):
            if operation == 'sources':
                return {'isError': False, 'outcome': {'sources': [{'source': {'kind': 'application'},
                        'requires_user_approval': False, 'available': True}]}}
            if operation == 'start':
                if role == 'outsider':
                    return {'isError': True, 'outcome': {'error': 'user approval'}}
                starts[0] += 1
                return {'isError': starts[0] > 1, 'outcome': {'error': 'already has' if starts[0] > 1 else None}}
            return {'isError': False, 'outcome': {}}
        original_write = whole_window.write
        def write(path, value):
            if path.name == 'measurement.json':
                captured.update(value); raise Captured()
            original_write(path, value)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'prepare.json').write_text(json.dumps({'contract': whole_window.CONTRACT,
                'preapproval_denied': True, 'receiver_id': 'Window-owned', 'candidate': {'pid': 1},
                'source_sha256': 'source', 'benchmark_sha256': 'benchmark'}))
            (root / 'lab.json').write_text(json.dumps({'vnc_address': '127.0.0.1:40000', 'launcher_pid': 2}))
            (root / 'closed.json').write_text('{"closed":true}')
            args = SimpleNamespace(prepared=root, viewer_evidence=root / 'viewer.json',
                resolution='1080p', orientation='landscape', backend='cpu', scaler=None, seconds=10)
            with patch.object(whole_window, 'validate_viewer'), \
                 patch.object(whole_window, 'verify_candidate', return_value={'pid': 1}), \
                 patch.object(whole_window, 'source_digest', return_value='source'), \
                 patch.object(whole_window, 'benchmark_digest', return_value='benchmark'), \
                 patch.object(whole_window, 'capture_reference'), patch.object(whole_window, 'request', side_effect=request), \
                 patch.object(whole_window, 'session', side_effect=status), patch.object(whole_window, 'wait', return_value=True), \
                 patch.object(whole_window, 'write', side_effect=write), patch.object(resources, 'Sampler', FakeSampler), \
                 patch.object(whole_window.time, 'monotonic', side_effect=lambda: now[0]), \
                 patch.object(whole_window.time, 'sleep', side_effect=lambda n: now.__setitem__(0, now[0] + n)):
                with self.assertRaises(Captured):
                    whole_window.run(args)
        self.assertEqual((captured['began_monotonic'], captured['ended_monotonic']), (6.25, 16.75))
        self.assertEqual(captured['seconds'], 10.5)
        self.assertEqual(captured['sender_counter_seconds'], 14.75)


if __name__ == '__main__':
    unittest.main()
