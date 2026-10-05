"""Frozen executable provenance rejects changes before trusting independent tools."""
import hashlib
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import whole_window
import window_decode
from window_fixture import Desktop
import window_provenance as provenance


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


class ExecutableProvenanceTests(unittest.TestCase):
    def decoders(self, root):
        records = {}
        for name in ('ffmpeg', 'ffprobe'):
            path = root / name; path.write_bytes(name.encode())
            records[name] = {'path': str(path), 'sha256': sha(path), 'version': 'fixture version'}
        return records

    def test_recorded_decoder_identity_uses_absolute_path_hash_and_version(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); expected = self.decoders(root)
            with patch.object(provenance.shutil, 'which', side_effect=lambda name: str(root / name)), \
                 patch.object(provenance.subprocess, 'check_output', return_value='fixture version\n') as execute:
                self.assertEqual(provenance.decoder_identities(), expected)
                self.assertEqual(provenance.verified_decoders(expected), {k: v['path'] for k, v in expected.items()})
                self.assertEqual([call.args[0] for call in execute.call_args_list],
                                 [[str(root / 'ffmpeg'), '-version'], [str(root / 'ffprobe'), '-version']])

    def test_changed_decoder_resolution_and_bytes_fail_before_invocation(self):
        for changed_path in (False, True):
            with self.subTest(changed_path=changed_path), tempfile.TemporaryDirectory() as directory:
                root = Path(directory); expected = self.decoders(root)
                selected = root / ('replacement' if changed_path else 'ffmpeg')
                selected.write_bytes(b'ffmpeg' if changed_path else b'changed decoder')
                with patch.object(provenance.shutil, 'which', side_effect=lambda name: str(selected if name == 'ffmpeg' else root / name)), \
                     patch.object(provenance.subprocess, 'check_output') as execute:
                    with self.assertRaisesRegex(RuntimeError, 'resolution changed|executable changed'):
                        provenance.verified_decoders(expected)
                    execute.assert_not_called()

    def test_changed_device_fails_before_independent_capture(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); (root / 'bin').mkdir(); device = root / 'bin/horizon-device'
            device.write_bytes(b'frozen'); expected = sha(device); device.write_bytes(b'changed')
            with patch.object(whole_window.subprocess, 'check_output') as execute:
                with self.assertRaisesRegex(RuntimeError, 'executable changed'):
                    whole_window.capture_reference(root, {}, {'pid': 1}, expected)
                execute.assert_not_called()

    def test_final_close_retains_original_prepare_hash(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); binaries = root / 'bin'; binaries.mkdir(); device = binaries / 'horizon-device'
            device.write_bytes(b'frozen'); expected = sha(device)
            with patch.object(Desktop, 'start'):
                desktop = Desktop(root, binaries, device_sha256=expected)
            device.write_bytes(b'changed')
            (root / 'prepare.json').write_text(json.dumps({'device_sha256': sha(device)}))
            with patch('window_fixture.closing_screenshot') as screenshot:
                with self.assertRaisesRegex(RuntimeError, 'executable changed'):
                    desktop.close_normally(device)
                screenshot.assert_not_called()
            self.assertEqual(desktop.device_sha256, expected)

    def test_explicit_decoder_paths_and_default_offline_interface(self):
        for tools in (None, {'ffmpeg': '/verified/ffmpeg', 'ffprobe': '/verified/ffprobe'}):
            with self.subTest(tools=tools), tempfile.TemporaryDirectory() as directory:
                result = SimpleNamespace(stdout=json.dumps({'streams': [{'width': 9, 'height': 9}]}))
                with patch.object(window_decode.subprocess, 'run', return_value=result) as execute:
                    with self.assertRaisesRegex(RuntimeError, 'canvas differs'):
                        window_decode.decode(Path(directory), {'stream_file': 'stream.h264'}, (1, 1), tools=tools)
                    self.assertEqual(execute.call_args.args[0][0], tools['ffprobe'] if tools else 'ffprobe')

    def test_reference_decode_uses_explicit_ffmpeg_path(self):
        with tempfile.TemporaryDirectory() as directory:
            image = Path(directory) / 'reference.png'; image.write_bytes(b'fixture')
            reference = {'image': str(image), 'sha256': sha(image), 'region': {'width': 1, 'height': 1}}
            with patch.object(window_decode.subprocess, 'check_output', return_value=b'') as execute:
                with self.assertRaisesRegex(RuntimeError, 'dimensions changed'):
                    window_decode.chrome_reference(reference, ffmpeg='/verified/ffmpeg')
                self.assertEqual(execute.call_args.args[0][0], '/verified/ffmpeg')


if __name__ == '__main__':
    unittest.main()
