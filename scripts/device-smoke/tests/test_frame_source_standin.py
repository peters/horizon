#!/usr/bin/env python3
"""The stand-in of a frame source program, against the contract that it speaks.

The tests read the bytes that the program writes and the messages that it reads,
without Horizon and without a desktop. A pair of Unix sockets and two pipes stand in
for the three streams of the contract.
"""

import json
import os
import select
import socket
import struct
import sys
import tempfile
import time
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCRIPT = HERE.parent / 'frame_source_standin.py'

MAGIC = b'NV12FRM1'


def frame(body):
    """One control message: four bytes of length, most significant byte first."""
    return struct.pack('>I', len(body)) + body


def message(value):
    """The bytes of one control message of a dict."""
    return frame(json.dumps(value).encode('utf-8'))


def read_message(stream):
    """The next control message of a stream, or None at its end."""
    length_bytes = stream.read(4)
    if len(length_bytes) < 4:
        return None
    length = struct.unpack('>I', length_bytes)[0]
    body = stream.read(length)
    if len(body) < length:
        return None
    return json.loads(body.decode('utf-8'))


def read_picture(stream, header=None):
    """The next picture of a stream: its header fields and its planes."""
    head = stream.read(32) if header is None else header
    if len(head) < 32:
        return None
    magic = head[:8]
    sequence, captured, width, height = struct.unpack('<QQII', head[8:32])
    planes = stream.read(width * height + width * height // 2)
    if len(planes) < width * height + width * height // 2:
        return None
    return {
        'magic': magic,
        'sequence': sequence,
        'captured': captured,
        'width': width,
        'height': height,
        'y': planes[: width * height],
        'uv': planes[width * height :],
    }


class StandinRun:
    """One stand-in process whose socket is its descriptor 3.

    The process starts through `os.posix_spawn`, whose file actions put the two
    pipes and the socket exactly where the contract names them, before the program
    runs. `subprocess` cannot do that: it closes the file descriptors that it did not
    name after the hook that a child would use to move one.
    """

    def __init__(self, *arguments):
        self.parent, child = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
        stdin_read, stdin_write = os.pipe()
        stdout_read, stdout_write = os.pipe()
        stderr_read, stderr_write = os.pipe()
        actions = [
            (os.POSIX_SPAWN_DUP2, stdin_read, 0),
            (os.POSIX_SPAWN_DUP2, stdout_write, 1),
            (os.POSIX_SPAWN_DUP2, stderr_write, 2),
            (os.POSIX_SPAWN_DUP2, child.fileno(), 3),
        ]
        self.pid = os.posix_spawn(
            sys.executable,
            [sys.executable, '-B', str(SCRIPT), *arguments],
            os.environ,
            file_actions=actions,
        )
        for descriptor in (stdin_read, stdout_write, stderr_write):
            os.close(descriptor)
        child.close()
        self.stdin = os.fdopen(stdin_write, 'wb', 0)
        self.stdout = os.fdopen(stdout_read, 'rb', 0)
        self.stderr = os.fdopen(stderr_read, 'rb', 0)
        self.reader = self.parent.makefile('rb')
        self.status = None

    def send(self, value):
        self.stdin.write(message(value))

    def next_message(self):
        return read_message(self.stdout)

    def next_message_within(self, seconds=5.0):
        ready, _, _ = select.select([self.stdout], [], [], seconds)
        if not ready:
            return None
        return self.next_message()

    def pictures(self, count):
        return [read_picture(self.reader) for _ in range(count)]

    def close_input(self):
        self.stdin.close()

    def close_socket(self):
        self.reader.close()
        self.parent.close()

    def wait(self, timeout=10):
        deadline = time.monotonic() + timeout
        while self.status is None:
            pid, status = os.waitpid(self.pid, os.WNOHANG)
            if pid:
                self.status = os.waitstatus_to_exitcode(status)
                break
            if time.monotonic() > deadline:
                raise TimeoutError('the stand-in did not end')
            time.sleep(0.01)
        return self.status

    def stop(self):
        if self.status is None:
            try:
                os.kill(self.pid, 9)
            except ProcessLookupError:
                pass
            os.waitpid(self.pid, 0)
            self.status = -9
        for stream in (self.reader, self.stdin, self.stdout, self.stderr, self.parent):
            try:
                stream.close()
            except (OSError, ValueError):
                pass


class FrameSourceStandinTests(unittest.TestCase):
    """The contract that the stand-in speaks, and the options of the fixture."""

    def setUp(self):
        self.runs = []

    def tearDown(self):
        for run in self.runs:
            run.stop()

    def start(self, *arguments):
        run = StandinRun(*arguments)
        self.runs.append(run)
        return run

    def test_the_hello_and_the_stream_come_before_the_first_picture(self):
        run = self.start('--width', '64', '--height', '32', '--rate', '0')
        hello = run.next_message()
        self.assertEqual(hello['type'], 'hello')
        self.assertEqual(hello['protocol'], 1)
        self.assertEqual(hello['formats'], ['nv12'])
        self.assertIn('source', hello)
        self.assertIn('session', hello)
        self.assertFalse(hello['input'])
        self.assertIn('input_reason', hello)
        stream = run.next_message()
        self.assertEqual(stream['type'], 'stream')
        self.assertEqual((stream['width'], stream['height']), (64, 32))
        self.assertEqual((stream['source_width'], stream['source_height']), (64, 32))
        first = run.pictures(1)[0]
        self.assertEqual(first['magic'], MAGIC)
        self.assertEqual((first['width'], first['height']), (64, 32))
        self.assertEqual(len(first['y']), 64 * 32)
        self.assertEqual(len(first['uv']), 64 * 32 // 2)

    def test_each_picture_carries_its_own_sequence_number_and_moves(self):
        run = self.start('--width', '64', '--height', '32', '--rate', '0')
        run.next_message()
        run.next_message()
        pictures = run.pictures(3)
        self.assertEqual([picture['sequence'] for picture in pictures], [0, 1, 2])
        self.assertTrue(all(picture['captured'] > 0 for picture in pictures))
        # The bar of the pattern moves with the sequence number, so two pictures
        # differ and a test sees that the newest one arrived.
        self.assertNotEqual(pictures[0]['y'], pictures[1]['y'])

    def test_the_program_ends_with_zero_when_its_input_ends_and_at_bye(self):
        for word in (None, {'type': 'bye'}):
            run = self.start('--rate', '0')
            run.next_message()
            run.next_message()
            if word is None:
                run.close_input()
            else:
                run.send(word)
            self.assertEqual(run.wait(), 0)

    def test_the_program_ends_with_zero_when_its_socket_closes(self):
        run = self.start('--rate', '0')
        run.next_message()
        run.next_message()
        run.pictures(1)
        run.close_socket()
        self.assertEqual(run.wait(), 0, 'a program whose viewer went ends by itself')

    def test_the_report_says_the_socket_is_descriptor_three(self):
        with tempfile.TemporaryDirectory() as directory:
            report = os.path.join(directory, 'report.json')
            run = self.start('--rate', '0', '--report', report)
            run.next_message()
            with open(report, encoding='utf-8') as file:
                record = json.loads(file.readline())
            self.assertEqual(record['pid'], run.pid)
            self.assertEqual(record['socket_fd'], 3, 'the socket is descriptor 3 of the child')
            self.assertTrue(record['socket'], 'descriptor 3 is one stream socket')
            self.assertFalse(record['input'])

    def test_a_view_that_changes_the_size_waits_for_the_old_size_pictures(self):
        run = self.start('--width', '128', '--height', '64', '--rate', '0', '--old-size-pictures', '2')
        run.next_message()
        run.next_message()
        run.pictures(1)
        run.send({'type': 'view', 'width': 64, 'height': 32})
        # Two pictures of the old size come, and only then the news of the new size.
        old = run.pictures(2)
        self.assertEqual([(picture['width'], picture['height']) for picture in old], [(128, 64), (128, 64)])
        stream = run.next_message()
        self.assertEqual(stream['type'], 'stream')
        self.assertEqual((stream['width'], stream['height']), (64, 32))
        self.assertEqual((stream['source_width'], stream['source_height']), (128, 64))
        new = run.pictures(1)[0]
        self.assertEqual((new['width'], new['height']), (64, 32))

    def test_a_view_above_the_source_asks_for_the_size_of_the_source(self):
        run = self.start('--width', '64', '--height', '32', '--rate', '0')
        run.next_message()
        run.next_message()
        run.pictures(1)
        run.send({'type': 'view', 'width': 4096, 'height': 4096})
        # The program captures 64 by 32 and never sends a larger picture.
        picture_of_run = run.pictures(1)[0]
        self.assertEqual((picture_of_run['width'], picture_of_run['height']), (64, 32))

    def test_a_view_that_hides_the_viewer_stops_the_pictures_and_keeps_the_counters(self):
        run = self.start('--width', '32', '--height', '16', '--rate', '50')
        run.next_message()
        run.next_message()
        run.pictures(1)
        run.send({'type': 'view', 'visible': False})
        self.assertEqual(run.next_message()['phase'], 'hidden')
        run.parent.settimeout(1.0)
        # The counters keep coming while the viewer is hidden, so an inspection of
        # the panel stays live. No picture comes.
        stats = None
        try:
            while stats is None:
                message_of_run = run.next_message()
                if message_of_run is None:
                    break
                if message_of_run['type'] == 'stats':
                    stats = message_of_run
        except (TimeoutError, socket.timeout):
            pass
        self.assertIsNotNone(stats, 'the counters of a hidden viewer come each second')
        self.assertEqual(stats['width'], 32)
        run.send({'type': 'view', 'visible': True})
        self.assertEqual(run.next_message()['phase'], 'live')

    def test_the_age_of_the_newest_picture_grows_while_the_viewer_is_hidden(self):
        run = self.start('--width', '32', '--height', '16', '--rate', '50')
        run.next_message()
        run.next_message()
        run.pictures(1)
        run.send({'type': 'view', 'visible': False})
        self.assertEqual(run.next_message_within()['phase'], 'hidden')
        ages = []
        while len(ages) < 2:
            message_of_run = run.next_message_within()
            self.assertIsNotNone(message_of_run, 'the counters of a hidden viewer come each second')
            if message_of_run['type'] == 'stats':
                ages.append(message_of_run['age_ms'])
        self.assertGreater(
            ages[1],
            ages[0],
            'no picture comes while the viewer is hidden, so its age grows',
        )
        self.assertGreaterEqual(ages[1], 900, 'the second counter is about a second older')

    def test_input_messages_are_written_to_the_file_of_the_run(self):
        with tempfile.TemporaryDirectory() as directory:
            log = os.path.join(directory, 'input.jsonl')
            run = self.start('--rate', '0', '--input', log)
            hello = run.next_message()
            self.assertTrue(hello['input'])
            run.next_message()
            run.send({'type': 'input', 'event': 'pointer', 'x': 3, 'y': 4})
            run.send({'type': 'input', 'event': 'key', 'key': 'a', 'down': True})
            deadline = 100
            while deadline and not (os.path.exists(log) and len(Path(log).read_text().splitlines()) >= 2):
                deadline -= 1
                import time

                time.sleep(0.02)
            lines = [json.loads(line) for line in Path(log).read_text().splitlines()]
            self.assertEqual(lines[0], {'type': 'input', 'event': 'pointer', 'x': 3, 'y': 4})
            self.assertEqual(lines[1], {'type': 'input', 'event': 'key', 'key': 'a', 'down': True})

    def test_a_failing_source_exits_with_one_with_one_line_of_text(self):
        run = self.start('--rate', '0', '--fail-after', '2')
        run.next_message()
        run.next_message()
        run.pictures(2)
        self.assertEqual(run.wait(), 1)
        self.assertIn(b'failed', run.stderr.read())

    def test_a_busy_source_answers_its_code_and_ends_with_zero(self):
        run = self.start('--busy')
        error = run.next_message()
        self.assertEqual(error['type'], 'error')
        self.assertEqual(error['code'], 'busy')
        self.assertTrue(error['message'])
        self.assertEqual(run.wait(), 0)

    def test_a_garbage_header_is_not_the_magic_of_the_contract(self):
        run = self.start('--rate', '0', '--garbage-header')
        run.next_message()
        run.next_message()
        head = run.parent.recv(32)
        self.assertEqual(len(head), 32)
        self.assertNotEqual(head[:8], MAGIC)

    def test_a_size_and_a_rate_that_break_the_contract_are_made_valid(self):
        run = self.start('--width', '17', '--height', '9', '--rate', '0')
        hello = run.next_message()
        self.assertEqual((hello['width'], hello['height']), (16, 16), 'an even side of 16 at the least')
        self.assertEqual(hello['formats'], ['nv12'])

    def test_an_unknown_message_and_text_that_is_not_json_are_no_command(self):
        run = self.start('--rate', '0')
        run.next_message()
        run.next_message()
        run.stdin.write(message({'type': 'something-new', 'x': 1}))
        run.stdin.write(frame(b'not json at all'))
        # The program still sends its pictures, so neither message was a command.
        self.assertIsNotNone(run.pictures(1)[0])


if __name__ == '__main__':
    unittest.main()
