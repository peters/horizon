#!/usr/bin/env python3
"""Self test for codex_attention_standin.py, the program of a codex panel.

The stand-in is the program of a codex panel of the fixture. It writes the state
of its turn into the title of its own terminal, and this test proves:

  * each typed word writes the title sequence that the real program writes, in
    the exact bytes of an OSC 0 title that ends with BEL;
  * a working turn keeps writing new spinner frames, and a repeated frame of the
    same state is a frame and not a new state;
  * both frames of an approval carry the request and the folder, and the words
    of the request are not read by Horizon;
  * a plain title names no frame and no request;
  * `spin N` writes the frames of the turn, and `plain SECONDS` writes a plain
    title and then the spinner of the same turn again;
  * a delayed line sends its word when the delay ends, and `quit` ends the
    program;
  * `--protocol` writes the reports of the program status protocol in the bytes
    of the real program, and a run without it writes no report at all.

Standard library only, no network. Nothing is written outside the folder of a
test.
"""
from __future__ import annotations

import os
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCRIPT = HERE.parent / 'codex_attention_standin.py'
PROGRAM = 'codex-attention-stand-in: '
FOLDER = 'work'
SPINNER = '\u280b'
# The frames of the braille spinner of a working turn.
BRAILLE_FRAMES = tuple(chr(code) for code in range(0x2800, 0x2900))
# The bounded wait of a test: a title is written at once, and the delay of the
# spinner of a working turn is a tenth of a second.
PATIENCE = 10.0


class Reader(threading.Thread):
    """The titles that the stand-in wrote, in the order it wrote them."""

    def __init__(self, stream):
        super().__init__(daemon=True)
        self.stream = stream
        self.titles = []
        self.raw = b''
        self.lock = threading.Lock()
        self.done = threading.Event()

    def run(self):
        marker = b'\x1b]0;'
        buffer = b''
        while True:
            chunk = self.stream.read(1)
            if not chunk:
                self.done.set()
                return
            with self.lock:
                self.raw += chunk
            buffer += chunk
            while True:
                start = buffer.find(marker)
                if start < 0:
                    # Keep the bytes that may begin the next sequence, and drop
                    # the text that a title is not.
                    buffer = buffer[-len(marker):]
                    while buffer and not marker.startswith(buffer):
                        buffer = buffer[1:]
                    break
                end = buffer.find(b'\x07', start)
                if end < 0:
                    buffer = buffer[start:]
                    break
                title = buffer[start + len(marker):end]
                buffer = buffer[end + 1:]
                with self.lock:
                    self.titles.append(title.decode('utf-8', 'replace'))

    def wait_for(self, count, timeout=PATIENCE):
        """Waits until the reader holds `count` titles, and answers them all."""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            with self.lock:
                if len(self.titles) >= count:
                    return list(self.titles)
            if self.done.is_set():
                break
            time.sleep(0.01)
        with self.lock:
            raise AssertionError(f'{count} titles did not arrive, these did: {self.titles!r}')

    def count(self):
        with self.lock:
            return len(self.titles)

    def wait_for_reports(self, count, timeout=PATIENCE):
        """Waits until `count` reports of the protocol arrived, and answers the raw bytes."""
        marker = b'\x1b]7501;'
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            with self.lock:
                raw = self.raw
            if raw.count(marker) >= count:
                return raw
            if self.done.is_set():
                break
            time.sleep(0.01)
        with self.lock:
            raise AssertionError(f'{count} reports did not arrive: {self.raw[-400:]!r}')


def reports_of(raw):
    """The payloads of the reports of the protocol, in the bytes that arrived.

    A report ends with ST, so a payload that reaches one is a whole report.
    """
    marker = b'\x1b]7501;'
    reports = []
    rest = raw
    while True:
        start = rest.find(marker)
        if start < 0:
            return reports
        end = rest.find(b'\x1b\\', start)
        if end < 0:
            return reports
        reports.append(rest[start + len(marker):end])
        rest = rest[end + 2:]


# The frames of the spinner of a launch of the real program, and the plain title
# that says it is ready, which follows them.
STARTUP_FRAMES = 8
STARTUP_TITLES = STARTUP_FRAMES + 1
# The reports of such a launch: one `state=working` with each frame and one
# `state=idle` at the moment the program is ready. A run without `--protocol`
# writes none of them.
STARTUP_REPORTS = STARTUP_FRAMES + 1


class StandIn:
    """The program of one panel, with the words of a person at its input."""

    def __init__(self, folder=FOLDER, protocol=False, arguments=(), ready=False):
        self.root = tempfile.TemporaryDirectory(prefix='codex-standin-')
        self.cwd = Path(self.root.name) / folder
        self.cwd.mkdir()
        self.arguments = [sys.executable, '-B', str(SCRIPT)]
        if protocol:
            self.arguments.append('--protocol')
        self.arguments.extend(arguments)
        if ready:
            self.arguments.append('--startup')
        self.child = subprocess.Popen(
            self.arguments,
            cwd=self.cwd,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        self.reader = Reader(self.child.stdout)
        self.reader.start()

    def word(self, text):
        assert self.child.stdin is not None
        self.child.stdin.write(f'{text}\n'.encode('utf-8'))
        self.child.stdin.flush()

    def close(self):
        if self.child.poll() is None:
            self.child.kill()
        self.child.wait(timeout=30)
        self.root.cleanup()


class CodexAttentionStandInTests(unittest.TestCase):
    def start(self, folder=FOLDER, protocol=False, arguments=(), ready=False):
        stand_in = StandIn(folder, protocol, arguments, ready)
        self.addCleanup(stand_in.close)
        return stand_in

    def test_the_launch_writes_the_frames_of_the_spinner_and_then_the_ready_title(self):
        stand_in = self.start(ready=True)
        titles = stand_in.reader.wait_for(STARTUP_TITLES, timeout=PATIENCE)

        for title in titles[:STARTUP_FRAMES]:
            self.assertIn(title[0], BRAILLE_FRAMES, title)
            self.assertTrue(title.endswith(f' {FOLDER}'), title)
        # The title of the moment the program is ready names no state at all.
        self.assertEqual(titles[STARTUP_TITLES - 1], FOLDER)

    def test_the_launch_reports_that_it_works_and_then_that_it_is_idle(self):
        stand_in = self.start(protocol=True, ready=True)
        raw = stand_in.reader.wait_for_reports(STARTUP_REPORTS)
        reports = reports_of(raw)

        self.assertEqual(reports[:STARTUP_FRAMES], [b'state=working:app=codex'] * STARTUP_FRAMES)
        self.assertEqual(reports[STARTUP_FRAMES], b'state=idle:app=codex')

    def test_a_launch_that_asks_a_question_blinks_it_before_it_is_ready(self):
        stand_in = self.start(protocol=True, ready=True, arguments=('--startup-question',))
        reports = reports_of(stand_in.reader.wait_for_reports(STARTUP_REPORTS + 1))
        self.assertEqual(reports[STARTUP_FRAMES], b'state=blocked:app=codex:kind=permission')
        self.assertEqual(reports[STARTUP_REPORTS], b'state=idle:app=codex')
        titles = stand_in.reader.wait_for(STARTUP_TITLES, timeout=PATIENCE)
        self.assertTrue(
            any(title.startswith('[ . ] Action Required | Do you trust') for title in titles),
            titles,
        )
        self.assertEqual(titles[-1], FOLDER)

    def test_a_denial_writes_a_plain_title_and_the_idle_report_of_a_stopped_turn(self):
        stand_in = self.start(protocol=True)
        stand_in.word('start')
        stand_in.reader.wait_for(1)
        stand_in.word('deny')
        titles = stand_in.reader.wait_for(2)
        raw = stand_in.reader.wait_for_reports(2)

        self.assertEqual(titles[1], FOLDER, 'no title of a turn that was denied')
        self.assertEqual(
            reports_of(raw)[-1],
            b'state=idle:app=codex',
            'a turn that an approval stopped reports idle and no done',
        )

    def test_a_denial_reports_the_wait_and_then_one_idle_and_nothing_between(self):
        stand_in = self.start(protocol=True)
        for word in ('start', 'ask', 'deny'):
            stand_in.word(word)
        reports = reports_of(stand_in.reader.wait_for_reports(3))

        self.assertEqual(
            reports,
            [
                b'state=working:app=codex',
                b'state=blocked:app=codex:kind=permission',
                b'state=idle:app=codex',
            ],
            'a denial writes the wait and one idle, with no working and no done between them',
        )
        stand_in.word('quit')
        self.assertEqual(stand_in.child.wait(timeout=30), 0)

    def test_an_answered_approval_that_a_turn_ends_reports_idle_after_working(self):
        stand_in = self.start(protocol=True)
        for word in ('start', 'ask', 'answer', 'esc'):
            stand_in.word(word)
        reports = reports_of(stand_in.reader.wait_for_reports(4))

        self.assertEqual(
            reports,
            [
                b'state=working:app=codex',
                b'state=blocked:app=codex:kind=permission',
                b'state=working:app=codex',
                b'state=idle:app=codex',
            ],
            'the answer takes the wait away and the interrupted turn reports idle and no done',
        )
        stand_in.word('quit')
        self.assertEqual(stand_in.child.wait(timeout=30), 0)

    def test_a_working_turn_writes_the_frames_of_its_spinner(self):
        stand_in = self.start()
        stand_in.word('start')
        titles = stand_in.reader.wait_for(2)

        self.assertEqual(titles[0], f'{SPINNER} {FOLDER}')
        self.assertTrue(all(title.endswith(f' {FOLDER}') for title in titles), titles)
        self.assertTrue(
            all(title[0] in '\u2800\u2801\u2802\u2803\u2804\u2805\u2806\u2807\u2808\u2809'
                '\u280a\u280b\u280c\u280d\u280e\u280f\u2819\u2839\u2838\u283c\u2834\u2826'
                '\u2827\u2807\u280f' for title in titles),
            f'the frames of a braille spinner: {titles}',
        )
        self.assertNotEqual(titles[0], titles[1], 'a working turn writes a new frame')

    def test_the_bytes_of_a_title_are_the_ones_the_program_writes(self):
        stand_in = self.start()
        stand_in.word('stop')
        stand_in.reader.wait_for(1)

        self.assertIn(b'\x1b]0;', stand_in.reader.raw, 'an OSC 0 title')
        self.assertIn(b'\x07', stand_in.reader.raw, 'a title that ends with BEL')

    def test_both_frames_of_an_approval_carry_the_request_and_the_folder(self):
        stand_in = self.start()
        stand_in.word('ask')
        titles = stand_in.reader.wait_for(2)

        self.assertEqual(
            titles[:2],
            [
                f'[ . ] Action Required | Create a file | {FOLDER}',
                f'[ ! ] Action Required | Create a file | {FOLDER}',
            ],
        )

    def test_an_answer_returns_to_the_spinner_of_the_same_turn(self):
        stand_in = self.start()
        stand_in.word('ask')
        stand_in.reader.wait_for(2)
        stand_in.word('answer')
        titles = stand_in.reader.wait_for(3)

        self.assertTrue(titles[2].endswith(f' {FOLDER}'), titles)
        self.assertNotIn('Action Required', titles[2])

    def test_a_plain_title_names_no_frame_and_no_request(self):
        for word in ('stop', 'esc'):
            stand_in = self.start()
            stand_in.word(word)
            stand_in.reader.wait_for(1)
            titles = list(stand_in.reader.titles)
            self.assertEqual(titles[0], FOLDER, word)
            self.assertNotIn('Action Required', titles[0])

    def test_spin_writes_the_frames_of_the_turn_at_once(self):
        stand_in = self.start()
        stand_in.word('spin 5')
        titles = stand_in.reader.wait_for(5)

        self.assertEqual(len(titles), 5, titles)
        self.assertTrue(all(title.endswith(f' {FOLDER}') for title in titles), titles)
        self.assertEqual(len({title for title in titles}), 5, 'each frame is its own title')

    def test_a_plain_title_between_two_parts_of_a_turn_returns_to_the_spinner(self):
        stand_in = self.start()
        stand_in.word('plain 0.1')
        titles = stand_in.reader.wait_for(2)

        self.assertEqual(titles[0], FOLDER, 'the plain title of the part between')
        self.assertTrue(titles[1].endswith(f' {FOLDER}'), 'the spinner of the turn comes back')
        self.assertFalse(titles[1].startswith('['), titles[1])

    def test_a_delayed_line_writes_its_word_when_the_delay_ends(self):
        stand_in = self.start()
        stand_in.word('after 0.2 stop')
        titles = stand_in.reader.wait_for(1, timeout=PATIENCE)

        self.assertEqual(titles[0], FOLDER)
        stand_in.word('quit')
        self.assertEqual(stand_in.child.wait(timeout=30), 0)

    def test_quit_writes_a_plain_title_and_ends_the_program(self):
        stand_in = self.start()
        stand_in.word('start')
        stand_in.reader.wait_for(1)
        stand_in.word('quit')
        self.assertEqual(stand_in.child.wait(timeout=30), 0)
        titles = list(stand_in.reader.titles)
        self.assertEqual(titles[-1], FOLDER, 'the program leaves a title with no state')

    def test_an_unknown_word_answers_one_line_and_the_program_still_ends(self):
        stand_in = self.start()
        stand_in.word('bogus')
        stand_in.word('quit')
        finished = stand_in.child.wait(timeout=30)
        errors = stand_in.child.stderr.read().decode('utf-8', 'replace')
        self.assertEqual(finished, 0, errors)

    def test_a_wrong_count_or_delay_answers_one_line_and_writes_no_title(self):
        stand_in = self.start()
        for text in ('spin 0', 'spin many', 'plain x', 'after 1', 'after 2 bogus'):
            stand_in.word(text)
        stand_in.word('quit')
        self.assertEqual(stand_in.child.wait(timeout=30), 0)
        titles = list(stand_in.reader.titles)
        self.assertEqual(titles, [FOLDER], 'only the plain title of quit is written')

    def test_the_protocol_argument_writes_the_reports_of_the_real_program(self):
        stand_in = self.start(protocol=True)
        for index, word in enumerate(('start', 'ask', 'answer', 'stop', 'fail', 'foreign')):
            stand_in.word(word)
            # The words before `foreign` write one report each; `stop` writes two.
            stand_in.reader.wait_for_reports(index + 1 + int(word == 'stop'))
        stand_in.word('quit')
        raw = stand_in.reader.wait_for_reports(8)
        self.assertEqual(stand_in.child.wait(timeout=30), 0)
        reports = reports_of(raw)
        self.assertEqual(
            reports,
            [
                b'state=working:app=codex',
                b'state=blocked:app=codex:kind=permission',
                b'state=working:app=codex',
                b'state=done:app=codex',
                b'state=idle:app=codex',
                b'state=error:app=codex',
                b'state=working:app=other',
                b'state=clear',
            ],
        )

    def test_a_run_without_the_argument_writes_no_report(self):
        stand_in = self.start()
        for word in ('start', 'ask', 'answer', 'stop', 'esc', 'fail', 'foreign'):
            stand_in.word(word)
        stand_in.reader.wait_for(6)
        stand_in.word('quit')
        self.assertEqual(stand_in.child.wait(timeout=30), 0)
        with stand_in.reader.lock:
            raw = stand_in.reader.raw
        self.assertNotIn(b'\x1b]7501;', raw, 'the titles alone are the panel of a real program')

    def test_work_writes_the_reports_of_a_working_turn_at_once(self):
        stand_in = self.start(protocol=True)
        stand_in.word('work 3')
        raw = stand_in.reader.wait_for_reports(3)
        stand_in.word('quit')
        self.assertEqual(stand_in.child.wait(timeout=30), 0)
        self.assertEqual(raw.count(b'\x1b]7501;state=working:app=codex'), 3)


if __name__ == '__main__':
    unittest.main()
