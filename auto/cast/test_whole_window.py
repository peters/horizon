"""Failure-injection gates for the distinct whole-window measurement contract."""
from pathlib import Path
import tempfile
import socket
import struct
import receiver
import json
import os
import time
import whole_window
import window_fixture
from types import SimpleNamespace

from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives import serialization
import auth
import window_decode
from unittest.mock import patch, MagicMock
import unittest

import window_resources as resources


def row(pid, start=100, own=10, reaped=0, rss=100):
    return {'pid': pid, 'start': start, 'self_ticks': own,
            'reaped_ticks': reaped, 'rss_kib': rss, 'state': 'S'}


class ResourceGates(unittest.TestCase):
    def test_reaped_child_counter_transfer_keeps_cpu_total(self):
        before = {1: row(1, own=10), 2: row(2, own=40)}
        after = {1: row(1, own=12, reaped=43)}
        self.assertEqual(resources.total_ticks(after) - resources.total_ticks(before), 5)

    def test_changed_topology_is_retried(self):
        parent, child = row(1), row(2)
        readings = [{1: parent, 2: child}, {1: parent}, {1: parent}, {1: parent}]
        with patch.object(resources, 'tree', side_effect=readings), patch.object(resources.time, 'sleep'):
            self.assertEqual(resources.stable_tree(1, 100), {1: parent})

    def test_cpu_transfer_during_snapshot_is_retried(self):
        readings = [{1: row(1, reaped=0)}, {1: row(1, reaped=4)},
                    {1: row(1, reaped=4)}, {1: row(1, reaped=4)}]
        with patch.object(resources, 'tree', side_effect=readings), patch.object(resources.time, 'sleep'):
            self.assertEqual(resources.stable_tree(1, 100)[1]['reaped_ticks'], 4)

    def test_recycled_application_pid_fails(self):
        with patch.object(resources, 'tree', return_value={1: row(1, start=200)}):
            with self.assertRaisesRegex(RuntimeError, 'identity'):
                resources.stable_tree(1, 100)

    def test_unstable_snapshot_never_becomes_zero_cpu(self):
        with patch.object(resources, 'tree', side_effect=FileNotFoundError), patch.object(resources.time, 'sleep'):
            with self.assertRaisesRegex(RuntimeError, 'stable'):
                resources.stable_tree(1, 100)

    def test_proc_stat_comm_with_spaces_and_parentheses(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / '23'; path.mkdir()
            fields = ['0'] * 22; fields[0] = 'S'
            for at, value in [(11, 3), (12, 4), (13, 5), (14, 6), (19, 81), (21, 2)]:
                fields[at] = str(value)
            (path / 'stat').write_text('23 (test (fixture)) ' + ' '.join(fields))
            value = resources.record(23, Path(directory))
            self.assertEqual((value['self_ticks'], value['reaped_ticks'], value['start']), (7, 11, 81))

    def test_gpu_query_includes_graphics_and_compute_only_owned_pids(self):
        xml = '<nvidia_smi_log><gpu><processes>' + ''.join(
            f'<process_info><pid>{pid}</pid><type>{kind}</type><used_memory>{amount} MiB</used_memory></process_info>'
            for pid, kind, amount in [(1, 'G', 20), (2, 'C', 30), (3, 'C+G', 40), (99, 'G', 1000)]) + '</processes></gpu></nvidia_smi_log>'
        self.assertEqual(resources.gpu_allocation(xml, {1, 2, 3}), 90)
        self.assertIsNone(resources.gpu_allocation(xml, {100}))

    def test_unavailable_gpu_memory_is_not_numeric_zero(self):
        xml = '<nvidia_smi_log><gpu><processes><process_info><pid>1</pid><type>G</type><used_memory>N/A</used_memory></process_info></processes></gpu></nvidia_smi_log>'
        with self.assertRaisesRegex(RuntimeError, 'unavailable'):
            resources.gpu_allocation(xml, {1})


class RememberedPairingGates(unittest.TestCase):
    def verify(self, registered=True, forged=False):
        controller = Ed25519PrivateKey.generate()
        key = controller.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)
        server = Ed25519PrivateKey.generate().public_key()
        server_bytes = server.public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)
        reference = object.__new__(auth.Reference)
        reference.shared_key = b's' * 32
        reference.client_ephemeral = b'e' * 32
        reference.controllers = {b'owned-controller': key} if registered else {}
        reference.keys = SimpleNamespace(verify_pub=server)
        signed = reference.client_ephemeral + b'owned-controller' + server_bytes
        signature = (Ed25519PrivateKey.generate() if forged else controller).sign(signed)
        inner = {auth.TlvValue.Identifier: b'owned-controller', auth.TlvValue.Signature: signature}
        with patch.object(auth, 'read_tlv', return_value=inner), patch.object(auth, 'Chacha20Cipher8byteNonce'), \
             patch.object(auth.BaseAirPlayServerAuth, '_m3_verify', return_value=b'accepted') as accepted:
            result = reference._m3_verify({auth.TlvValue.EncryptedData: b'fixture'})
            self.assertEqual(accepted.call_count, 1)
            return result

    def test_remembered_controller_signature_is_verified(self):
        self.assertEqual(self.verify(), b'accepted')

    def test_unknown_controller_cannot_verify(self):
        with self.assertRaises(KeyError):
            self.verify(registered=False)

    def test_forged_remembered_signature_cannot_verify(self):
        with self.assertRaises(InvalidSignature):
            self.verify(forged=True)


class WindowImageGates(unittest.TestCase):
    width, height = 400, 200
    marker = (20, 30, 10, 16)

    def fixture(self, identifier=123):
        pixels = bytearray([80] * (self.width * self.height * 3))
        def fill(left, top, width, height, value):
            for y in range(top, top + height):
                for x in range(left, left + width):
                    at = (y * self.width + x) * 3
                    pixels[at:at + 3] = bytes([value] * 3)
        values = [255, 0] * 3 + [128]
        values += [255 if identifier & (1 << bit) else 0 for bit in range(16)]
        values += [128] + [0, 255] * 3
        for column, value in enumerate(values):
            fill(20 + column * 10, 22, 10, 16, value)
        for row in range(2):
            for column in range(8):
                fill(20 + column * 20, 46 + row * 16, 20, 16,
                     30 if (column + row) % 2 == 0 else 180)
        return pixels

    def test_actual_fixture_marker_is_located_and_decoded(self):
        frame = self.fixture()
        marker = window_decode.locate(frame, self.width, self.height)
        identifier, errors = window_decode.inspect_frame(frame, self.width, self.height, marker)
        self.assertEqual(identifier, 123)
        self.assertEqual(max(errors), 0)

    def test_fractional_scaled_cells_use_independent_reference_width(self):
        original = self.fixture(); width, height = 183, 92
        frame = b''.join(original[(int((y + .5) * self.height / height) * self.width + int((x + .5) * self.width / width)) * 3:][:3]
                         for y in range(height) for x in range(width))
        marker = window_decode.locate(frame, width, height, 10 * width / self.width)
        self.assertEqual(window_decode.inspect_frame(frame, width, height, marker)[0], 123)
        with self.assertRaises(RuntimeError):
            window_decode.inspect_frame(frame, width, height, window_decode.locate(frame, width, height))

    def test_chrome_cell_preserves_area_contrast_and_clamps_boundaries(self):
        frame = bytes([20] * 27 + [200] * 27)
        self.assertEqual([window_decode.chrome_cell(frame, 3, 1, y, 1) for y in (0, 5, 2)], [20, 200, 80])

    def test_changed_checker_quality_is_rejected(self):
        frame = self.fixture()
        for y in range(46, 78):
            for x in range(20, 180):
                at = (y * self.width + x) * 3
                frame[at:at + 3] = bytes([90] * 3)
        with self.assertRaisesRegex(RuntimeError, 'quality'):
            window_decode.inspect_frame(frame, self.width, self.height, self.marker)

    def test_out_of_bounds_marker_never_qualifies(self):
        frame = self.fixture()
        with self.assertRaisesRegex(RuntimeError, 'outside'):
            window_decode.inspect_frame(frame, self.width, self.height, (400, 30, 10, 16))

    def test_grayscale_background_without_fixture_is_rejected(self):
        with self.assertRaisesRegex(RuntimeError, 'absent'):
            window_decode.locate(bytes([80] * self.width * self.height * 3), self.width, self.height)


class ReceiverRecordGates(unittest.TestCase):
    def receive(self, frame=None):
        with tempfile.TemporaryDirectory() as directory:
            reference = object.__new__(receiver.Receiver)
            reference.root = Path(directory)
            reference.clock_id = 4242
            reference.split_streams = True
            reference.errors = []
            reference.track = lambda connection: True
            state = {'frames': 0, 'configs': 0, 'stream_file': 'received-1.h264'}
            incoming, outgoing = socket.socketpair()
            listener = SimpleNamespace(accept=lambda: (incoming, None))
            configuration = bytes([1, 0, 0, 0, 255, 225]) + struct.pack('>H', 2) + b'\x67x' + bytes([1]) + struct.pack('>H', 2) + b'\x68y'
            def packet(kind, payload, stamp=0):
                header = bytearray(128)
                struct.pack_into('<I', header, 0, len(payload)); header[4] = kind
                struct.pack_into('<Q', header, 8, stamp)
                struct.pack_into('<Q', header, 40, 4242)
                return header + payload
            outgoing.sendall(packet(1, configuration))
            if frame is not None:
                outgoing.sendall(packet(0, frame, 1))
            outgoing.close()
            cipher = SimpleNamespace(decrypt=lambda nonce, body, header: body)
            with patch.object(receiver, 'ChaCha20Poly1305', return_value=cipher):
                reference.video(listener, b's' * 32, 1, state)
            return state, reference.errors, (Path(directory) / state['stream_file']).read_bytes()

    def test_configuration_without_picture_remains_separate(self):
        state, errors, stream = self.receive()
        self.assertFalse(errors)
        self.assertEqual(stream, b'')
        self.assertEqual(state['pending_configuration_bytes'], 12)
        self.assertEqual(state['frames'], 0)

    def test_complete_picture_flushes_configuration(self):
        state, errors, stream = self.receive(struct.pack('>I', 2) + b'\x65z')
        self.assertFalse(errors)
        self.assertEqual(stream, b'\x00\x00\x00\x01\x67x\x00\x00\x00\x01\x68y\x00\x00\x00\x01\x65z')
        self.assertEqual(state['frames'], 1)
        self.assertEqual(state['pending_configuration_bytes'], 0)

    def test_truncated_picture_records_receiver_error_and_no_frame(self):
        state, errors, stream = self.receive(struct.pack('>I', 5) + b'\x65z')
        self.assertTrue(errors)
        self.assertEqual(state['frames'], 0)


class NativeViewerGates(unittest.TestCase):
    def validate(self, mutate=None):
        now = int(time.time() * 1000)
        observations = [{'panels': [{'endpoint': '127.0.0.1:40000', 'panel_id': 'owned',
            'owned_by_caller': True, 'connection': 'connected', 'image_received': True,
            'image_displayed': True, 'frame_sequence': index + 1,
            'diagnostics': {'connection_generation': 1, 'observed_at_millis': now - (2 - index) * 2000}}]}
            for index in range(3)]
        if mutate:
            mutate(observations)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'viewer.json'; path.write_text(json.dumps(observations))
            return whole_window.validate_viewer(path, '127.0.0.1:40000')

    def test_current_owned_presented_moving_viewer_qualifies(self):
        self.assertEqual(self.validate()['panel_id'], 'owned')

    def test_received_but_unpresented_viewer_is_rejected(self):
        with self.assertRaisesRegex(RuntimeError, 'presentation'):
            self.validate(lambda rows: rows[0]['panels'][0].update(image_displayed=False))

    def test_other_owner_viewer_is_rejected(self):
        with self.assertRaisesRegex(RuntimeError, 'ownership'):
            self.validate(lambda rows: rows[0]['panels'][0].update(owned_by_caller=False))

    def test_stale_viewer_receipt_is_rejected(self):
        with self.assertRaisesRegex(RuntimeError, 'stale'):
            self.validate(lambda rows: [row['panels'][0]['diagnostics'].update(observed_at_millis=1 + i * 2000) for i, row in enumerate(rows)])

    def test_static_viewer_receipt_cannot_prove_live_motion(self):
        with self.assertRaisesRegex(RuntimeError, 'motion'):
            self.validate(lambda rows: [row['panels'][0].update(frame_sequence=1) for row in rows])


class CandidateIdentityGates(unittest.TestCase):
    def test_gui_requires_the_exact_private_config_and_ephemeral_mode(self):
        config = Path('/private/owned/data/horizon.yaml')
        self.assertTrue(window_fixture.application_arguments(['horizon', '--config', str(config), '--ephemeral'], config))
        for argv in [['horizon', '--browser-mcp'],
                     ['horizon', '--config', '/different/horizon.yaml', '--ephemeral'],
                     ['horizon', '--config', str(config)],
                     ['horizon', '--config', str(config), '--ephemeral', '--browser-mcp']]:
            with self.subTest(argv=argv):
                self.assertFalse(window_fixture.application_arguments(argv, config))


class ReviewRegressionGates(unittest.TestCase):
    def test_gpu_join_cannot_extend_cpu_interval_or_count_late_allocation(self):
        sampler = object.__new__(resources.Sampler)
        sampler.thread = MagicMock(); sampler.thread.is_alive.return_value = False
        sampler.done = MagicMock(); sampler.end = None; sampler.started = 1
        sampler.begin = {1: row(1, own=10)}; sampler.snapshot = lambda: {1: row(1, own=20)}
        sampler.identity = {}; sampler.errors = []; sampler.gpu_errors = []; sampler.gpu = True
        sampler.readings = [{'tree_rss_kib': 100, 'gpu_memory_mib': 40, 'gpu_observed_seconds': .5},
                            {'tree_rss_kib': 120, 'gpu_memory_mib': 999, 'gpu_observed_seconds': 5}]
        with patch.object(resources.time, 'monotonic', side_effect=[2, 9]):
            result = sampler.finish()
        self.assertEqual(result['seconds'], 1)
        self.assertEqual(result['gpu_memory_mib'], 40)
        self.assertIsNone(result['score'])

    def test_partial_mcp_line_obeys_deadline_and_coalesced_lines_remain_buffered(self):
        read, write = os.pipe()
        try:
            with os.fdopen(read, 'rb', buffering=0) as stream:
                buffer = bytearray(); os.write(write, b'{"id":1}\n{"id":2}\n')
                self.assertEqual(whole_window.read_json_line(stream, buffer, time.monotonic() + 1)['id'], 1)
                self.assertEqual(whole_window.read_json_line(stream, buffer, time.monotonic() + 1)['id'], 2)
                os.write(write, b'{"id":3')
                with self.assertRaises(TimeoutError):
                    whole_window.read_json_line(stream, buffer, time.monotonic() + .02)
        finally:
            os.close(write)

    def test_failed_mcp_initialization_reaps_process(self):
        process = MagicMock()
        with patch.object(whole_window.subprocess, 'Popen', return_value=process), \
             patch.object(whole_window.Mcp, 'call', side_effect=RuntimeError('initialize')):
            with self.assertRaisesRegex(RuntimeError, 'initialize'):
                whole_window.Mcp(Path('/fixture/horizon'), None)
        process.stdin.close.assert_called_once(); process.wait.assert_called_once()

    def test_preflight_failure_and_cancellation_finish_owned_fixture(self):
        for error in [RuntimeError('viewer invalid'), KeyboardInterrupt()]:
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / 'prepare.json').write_text(json.dumps({'contract': whole_window.CONTRACT,
                    'preapproval_denied': True, 'receiver_id': 'Window-synthetic'}))
                (root / 'lab.json').write_text(json.dumps({'vnc_address': '127.0.0.1:40000'}))
                (root / 'closed.json').write_text('{"closed":true}')
                with patch.object(whole_window, 'validate_viewer', side_effect=error), \
                     patch.object(whole_window, 'request') as request, self.assertRaises(type(error)):
                    whole_window.run(SimpleNamespace(prepared=root, viewer_evidence=root / 'missing'))
                self.assertTrue((root / 'finish').exists())
                self.assertEqual(json.loads((root / 'run-failure.json').read_text())['status'], 'FAIL')
                request.assert_called_once_with(root, 'owner', 'stop', receiver_id='Window-synthetic')

    def test_only_movement_outside_fixed_interval_cannot_qualify(self):
        state = {'frame_received_times': [0, 1, 2, 3, 4]}
        measurement = {'began_monotonic': 1, 'ended_monotonic': 3}
        with self.assertRaisesRegex(RuntimeError, 'frozen'):
            window_decode.measurement_ids([1, 2, 2, 2, 3], state, measurement)
        self.assertEqual(window_decode.measurement_ids([1, 2, 3, 4, 5], state, measurement), [2, 3, 4])

    def test_emitted_rows_erase_initial_wrapping_and_use_crlf(self):
        value = window_fixture.fixture_output(5)
        self.assertEqual(value.count('\033[2K'), 12)
        self.assertEqual(value.count('\r\n'), 11)
        self.assertTrue(value.endswith('\033[0m\033[J'))


class RootClientCoordinatesTests(unittest.TestCase):
    def test_absolute_client_coordinates_exclude_reparent_offsets(self):
        from window_fixture import root_region
        geometry = 'Absolute upper-left X: 51\nAbsolute upper-left Y: 62\nRelative upper-left X: 1\nRelative upper-left Y: 22\nWidth: 1500\nHeight: 920\n'
        self.assertEqual(root_region(geometry), {'x': 51, 'y': 62, 'width': 1500, 'height': 920})


class ClosingScreenshotTests(unittest.TestCase):
    def test_retries_only_busy_without_replaying_input(self):
        busy = SimpleNamespace(returncode=1, stdout=json.dumps({'ok': False, 'error': {'message': 'device busy; another command is active'}}))
        okay = SimpleNamespace(returncode=0, stdout=json.dumps({'ok': True, 'result': {}}))
        with patch('window_fixture.subprocess.run', side_effect=[busy, okay]) as run, patch('window_fixture.time.sleep'):
            self.assertTrue(window_fixture.closing_screenshot(['device', 'screenshot'])['ok'])
            self.assertEqual(run.call_count, 2)

    def test_other_failure_is_not_retried(self):
        bad = SimpleNamespace(returncode=1, stdout=json.dumps({'ok': False, 'error': {'message': 'expired target'}}))
        with patch('window_fixture.subprocess.run', return_value=bad) as run:
            with self.assertRaises(RuntimeError):
                window_fixture.closing_screenshot(['device', 'screenshot'])
            self.assertEqual(run.call_count, 1)


if __name__ == '__main__':
    unittest.main()
