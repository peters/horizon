"""The stand-in recorder and the stand-in service of a speech test.

One test starts the recorder, the other sends a request to the service, and neither
opens a microphone, a loudspeaker or a real service.
"""
from __future__ import annotations

import json
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest
import urllib.request

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import speech_standin_recorder  # noqa: E402
import speech_standin_service  # noqa: E402

# A WAV header and some samples: the stand-in copies the bytes of a recording.
PREPARED_WAV = b'RIFF$\x00\x00\x00WAVEfmt ' + bytes(64)


class StandInRecorderTests(unittest.TestCase):
    def test_the_recorder_copies_the_prepared_file_and_ends_on_sigint(self):
        with tempfile.TemporaryDirectory() as folder:
            folder = Path(folder)
            source = folder / 'prepared.wav'
            source.write_bytes(PREPARED_WAV)
            take = folder / 'take-standin.wav'
            process = subprocess.Popen(
                [sys.executable, '-B', str(Path(speech_standin_recorder.__file__)),
                 str(take), '--source', str(source), '--seconds', '30'])
            try:
                for _ in range(400):
                    if take.is_file():
                        break
                    process.poll()
                    self.assertIsNone(process.poll(), 'the recorder ended before its signal')
                    time.sleep(0.01)
                self.assertEqual(take.read_bytes(), PREPARED_WAV, 'the take holds the recording of the stand-in')
                # The exact child of this test, and no group: the signal goes to its own pid.
                process.send_signal(signal.SIGINT)
                self.assertEqual(process.wait(timeout=10), 0, 'a recorder ends cleanly on SIGINT')
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait(timeout=10)

    def test_a_source_that_is_not_a_file_is_refused(self):
        with tempfile.TemporaryDirectory() as folder:
            folder = Path(folder)
            done = subprocess.run(
                [sys.executable, '-B', str(Path(speech_standin_recorder.__file__)),
                 str(folder / 'take.wav'), '--source', str(folder / 'missing.wav')],
                capture_output=True, text=True, timeout=30)
            self.assertEqual(done.returncode, 2)
            self.assertIn('is not a file', done.stderr)
            self.assertFalse((folder / 'take.wav').exists())

    def test_the_copy_and_wait_answers_and_ends_after_its_bound(self):
        with tempfile.TemporaryDirectory() as folder:
            folder = Path(folder)
            source = folder / 'prepared.wav'
            source.write_bytes(PREPARED_WAV)
            take = folder / 'take.wav'
            self.assertEqual(speech_standin_recorder.copy_and_wait(take, source, 0.05), '')
            self.assertEqual(take.read_bytes(), PREPARED_WAV, 'a bounded wait copies the file too')


class StandInServiceTests(unittest.TestCase):
    def post(self, address, field_text='payload', authorization='Bearer a-value'):
        boundary = 'standin-boundary'
        body = (
            f'--{boundary}\r\nContent-Disposition: form-data; name="model"\r\n\r\n'
            f'gpt-transcribe\r\n--{boundary}\r\nContent-Disposition: form-data; name="response_format"'
            f'\r\n\r\njson\r\n--{boundary}\r\nContent-Disposition: form-data; name="file";'
            f' filename="take-1.wav"\r\nContent-Type: audio/wav\r\n\r\n{field_text}'
            f'\r\n--{boundary}--\r\n'
        ).encode()
        request = urllib.request.Request(
            f'http://{address[0]}:{address[1]}{speech_standin_service.TRANSCRIPTION_PATH}',
            data=body, method='POST',
            headers={'Content-Type': f'multipart/form-data; boundary={boundary}',
                     'Authorization': authorization})
        with urllib.request.urlopen(request, timeout=20) as answer:
            return json.loads(answer.read())

    def test_the_service_answers_the_text_and_records_no_key(self):
        with tempfile.TemporaryDirectory() as folder:
            record = Path(folder) / 'received.json'
            address, thread = speech_standin_service.serve(('127.0.0.1', 0), 'hei', record, 1)
            answer = self.post(address)
            thread.join(timeout=20)
            self.assertEqual(answer, {'text': 'hei'})
            kept = json.loads(record.read_text())
            self.assertEqual(kept['authorization'], 'redacted', 'the record says that a key came')
            self.assertEqual(kept['method'], 'POST')
            self.assertEqual(kept['path'], speech_standin_service.TRANSCRIPTION_PATH)
            self.assertEqual(kept['bytes'], len(self.last_body()), 'the record holds the bytes of the request')
            self.assertEqual(kept['fields'], ['model', 'response_format', 'file'])
            text = record.read_text()
            self.assertNotIn('a-value', text, 'the record holds no value of the key')
            self.assertNotIn('Authorization', text, 'the record holds no credential header')

    def last_body(self):
        boundary = 'standin-boundary'
        return (
            f'--{boundary}\r\nContent-Disposition: form-data; name="model"\r\n\r\n'
            f'gpt-transcribe\r\n--{boundary}\r\nContent-Disposition: form-data; name="response_format"'
            f'\r\n\r\njson\r\n--{boundary}\r\nContent-Disposition: form-data; name="file";'
            f' filename="take-1.wav"\r\nContent-Type: audio/wav\r\n\r\npayload'
            f'\r\n--{boundary}--\r\n'
        ).encode()

    def test_a_path_that_is_not_the_transcription_endpoint_answers_404(self):
        with tempfile.TemporaryDirectory() as folder:
            record = Path(folder) / 'received.json'
            address, thread = speech_standin_service.serve(('127.0.0.1', 0), 'hei', record, 1)
            request = urllib.request.Request(
                f'http://{address[0]}:{address[1]}/v1/other', data=b'--x--\r\n', method='POST',
                headers={'Content-Type': 'multipart/form-data; boundary=x'})
            with self.assertRaises(urllib.error.HTTPError) as refused:
                urllib.request.urlopen(request, timeout=20)
            thread.join(timeout=20)
            self.assertEqual(refused.exception.code, 404)
            refused.exception.close()

    def test_the_fields_of_a_body_and_its_boundary_are_read(self):
        body = (b'--b\r\nContent-Disposition: form-data; name="model"\r\n\r\nm'
                b'\r\n--b\r\nContent-Disposition: form-data; name="file"; filename="a.wav"\r\n\r\nx\r\n--b--\r\n')
        self.assertEqual(speech_standin_service.field_names(body, b'b'), ['model', 'file'])
        self.assertEqual(speech_standin_service.boundary_of('multipart/form-data; boundary="b"'), b'b')
        self.assertIsNone(speech_standin_service.boundary_of('application/json'))


if __name__ == '__main__':
    unittest.main()
