#!/usr/bin/env python3
"""Self test for pi_attention_standin.py, with a recorder as the hook entry.

The stand-in is the program of a pi panel of the fixture. Horizon gives such a
panel `-e <report>` and the five `HORIZON_WORK_*` variables of its launch, and
this test proves:

  * the `-e` file of the launch is read, and a launch without one sends nothing;
  * each typed word sends its documents to
    `<HORIZON_WORK_EXECUTABLE> --agent-work-hook`, in the environment of the
    launch, and a turn closes with the id that opened it;
  * a delayed line sends its word when the delay ends, so the person can move the
    focus away before the turn ends;
  * the documents of the fallback carry no prompt and no tool content, and their
    paths are absolute;
  * without the work variables it says one line and still answers `quit`.

The run with `node` proves the real report through the harness of this folder and
is skipped when this computer has no Node; the Rust test
`crates/horizon-ui/tests/pi_report_extension.rs` proves the report itself.

Standard library only. Nothing is written outside the folder of a test.
"""
from __future__ import annotations

import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCRIPT = HERE.parent / 'pi_attention_standin.py'
HARNESS = HERE.parent / 'pi_extension_harness.mjs'
REPORT = HERE.parents[2] / 'assets' / 'plugins' / 'pi' / 'horizon-report.mjs'
MODULE = 'pi_attention_standin'
MODULE_PREFIX = 'pi-attention-stand-in: '
WORK_VARIABLES = (
    'HORIZON_WORK_ROOT',
    'HORIZON_WORK_PANEL',
    'HORIZON_WORK_OWNER',
    'HORIZON_WORK_KIND',
    'HORIZON_WORK_EXECUTABLE',
)
# The recorder takes the place of `<candidate> --agent-work-hook`: it keeps the
# arguments and the document that the stand-in sent it.
RECORDER = '''#!/usr/bin/env python3
import json, os, sys
folder = os.path.dirname(os.path.abspath(__file__))
record = {'arguments': sys.argv[1:], 'input': sys.stdin.read()}
with open(os.path.join(folder, 'calls.jsonl'), 'a', encoding='utf-8') as calls:
    calls.write(json.dumps(record) + '\\n')
'''


def load():
    """Import the stand-in without leaving a __pycache__ in the fixture folder."""
    sys.dont_write_bytecode = True
    if MODULE not in sys.modules:
        sys.path.insert(0, str(SCRIPT.parent))
        import pi_attention_standin  # noqa: F401  (imported for its functions)

    return sys.modules[MODULE]


def recorder(directory):
    path = directory / 'recorder'
    path.write_text(RECORDER)
    path.chmod(path.stat().st_mode | stat.S_IXUSR)
    return path


def environment(directory, executable, with_variables=True):
    values = {name: value for name, value in os.environ.items() if name not in WORK_VARIABLES}
    if with_variables:
        values.update({
            'HORIZON_WORK_ROOT': str(directory / 'home'),
            'HORIZON_WORK_PANEL': 'panel-pi',
            'HORIZON_WORK_OWNER': 'launch-owner',
            'HORIZON_WORK_KIND': 'pi',
            'HORIZON_WORK_EXECUTABLE': str(executable),
        })
    return values


def calls(directory):
    path = directory / 'calls.jsonl'
    if not path.is_file():
        return []
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]


def wait_for_calls(directory, count, seconds=20):
    """The calls of the entry, once `count` of them arrived.

    The report of a pi launch starts its entry as a detached process that it never
    joins, so the last call of a run can arrive after the stand-in ended.
    """
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline and len(calls(directory)) < count:
        time.sleep(0.02)
    return calls(directory)


def documents(directory):
    return [json.loads(call['input']) for call in calls(directory)]


class StandInTests(unittest.TestCase):

    def test_a_word_sends_its_document_and_a_turn_closes_with_its_own_id(self):
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder)
            finished = subprocess.run(
                [sys.executable, str(SCRIPT), '-e', str(REPORT)],
                input='start\nask\nanswer\nstop\nstart\nesc\nquit\n', text=True, capture_output=True,
                check=False, env=environment(directory, recorder(directory)), timeout=120)
            self.assertEqual(finished.returncode, 0, finished.stderr)
            wait_for_calls(directory, 7)
            events = documents(directory)
            # Each word fires its own call, and the report starts the entry as a
            # detached process that it never joins, so the order of two calls of
            # neighbouring words is not the order of the words. The events and the
            # turn of each of them are the rule.
            self.assertEqual(sorted(event['hook_event_name'] for event in events),
                             sorted(['UserPromptSubmit', 'PermissionRequest', 'PostToolUse', 'Stop',
                                     'UserPromptSubmit', 'Interrupt', 'SessionEnd']))
            names = {}
            for event in events:
                names.setdefault(event['hook_event_name'], []).append(event['prompt_id'])
            submits = names['UserPromptSubmit']
            self.assertEqual(len(submits), 2, 'each busy span opens a turn of its own')
            self.assertNotEqual(submits[0], submits[1], 'the two turns have their own ids')
            self.assertEqual(names['Stop'], [submits[0]], 'the stop closes the turn that it opened')
            self.assertEqual(names['Interrupt'], [submits[1]], 'the interrupt closes the second turn')
            for name in ('PermissionRequest', 'PostToolUse'):
                self.assertIn(names[name][0], submits, f'the {name} names the turn of its word')
            self.assertEqual(names['SessionEnd'], [None], 'the end of the session names no turn')
            for call in calls(directory):
                self.assertEqual(call['arguments'], ['--agent-work-hook'])
            for event in events:
                self.assertTrue(os.path.isabs(event['cwd']))
                self.assertTrue(os.path.isabs(event['transcript_path']))
                self.assertNotIn('prompt', event)
                self.assertNotIn('tool_input', event)

    def test_a_launch_without_the_report_sends_nothing(self):
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder)
            finished = subprocess.run(
                [sys.executable, str(SCRIPT)],
                input='start\nquit\n', text=True, capture_output=True, check=False,
                env=environment(directory, recorder(directory)), timeout=120)
            self.assertEqual(finished.returncode, 0, finished.stderr)
            self.assertIn('named no report file with -e', finished.stdout)
            self.assertEqual(calls(directory), [], 'a launch without a report claims nothing')

    def test_without_the_work_variables_it_says_one_line_and_still_answers_quit(self):
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder)
            finished = subprocess.run(
                [sys.executable, str(SCRIPT), '-e', str(REPORT)],
                input='start\nquit\n', text=True, capture_output=True, check=False,
                env=environment(directory, recorder(directory), with_variables=False), timeout=120)
            self.assertEqual(finished.returncode, 0, finished.stderr)
            self.assertIn('HORIZON_WORK_ROOT', finished.stdout.splitlines()[0])
            self.assertIn('quit fired', finished.stdout)
            self.assertEqual(calls(directory), [])

    def test_a_delayed_line_sends_its_word_when_the_delay_ends(self):
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder)
            began = time.monotonic()
            answer = subprocess.run(
                [sys.executable, str(SCRIPT), '-e', str(REPORT)],
                input='start\nafter 1 stop\nafter 3 quit\n', text=True, capture_output=True,
                check=False, env=environment(directory, recorder(directory)), timeout=120)
            took = time.monotonic() - began
            self.assertEqual(answer.returncode, 0, answer.stderr)
            wait_for_calls(directory, 3)
            events = documents(directory)
            self.assertEqual([event['hook_event_name'] for event in events],
                             ['UserPromptSubmit', 'Stop', 'SessionEnd'])
            self.assertGreaterEqual(took, 3, 'the delayed words wait for their delay')

    def test_the_delay_of_a_line_is_read_and_bounded(self):
        stand_in = load()
        self.assertEqual(stand_in.parse_delay('after 2 stop'), (2.0, 'stop'))
        for line, reason in (('after stop', 'write "after SECONDS WORD"'),
                             ('after x stop', 'is no number of seconds'),
                             ('after -1 stop', 'outside 0 to'),
                             ('after 1000 stop', 'outside 0 to'),
                             ('after 1 sing', 'unknown word')):
            with self.assertRaises(ValueError) as raised:
                stand_in.parse_delay(line)
            self.assertIn(reason, str(raised.exception))
        self.assertIsNone(stand_in.parse_delay('start'))

    def test_a_computer_without_node_sends_the_documents_of_the_report(self):
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder)
            values = environment(directory, recorder(directory))
            # A PATH without Node, which is the case that this fallback serves.
            values['PATH'] = '/usr/bin:/bin'
            finished = subprocess.run(
                [sys.executable, str(SCRIPT), '-e', str(REPORT)],
                input='start\nask\nanswer\nstop\nquit\n', text=True, capture_output=True,
                check=False, env=values, timeout=120)
            self.assertEqual(finished.returncode, 0, finished.stderr)
            self.assertIn('no node on PATH', finished.stdout)
            wait_for_calls(directory, 5)
            # The report starts the entry of each event as a detached process, so
            # the events of two words can arrive in the order of the processes and
            # not in the order of the words.
            self.assertEqual(sorted(event['hook_event_name'] for event in documents(directory)),
                             sorted(['UserPromptSubmit', 'PermissionRequest', 'PostToolUse', 'Stop',
                                     'SessionEnd']))
            for event in documents(directory):
                self.assertNotIn('prompt', event)
                self.assertTrue(os.path.isabs(event['cwd']))

    def test_the_report_file_of_the_launch_is_the_argument_of_e(self):
        stand_in = load()
        self.assertEqual(stand_in.report_file(['-e', '/tmp/report.mjs', '--session', 'x']),
                         '/tmp/report.mjs')
        self.assertIsNone(stand_in.report_file(['--plugin-dir', 'x']))

    @unittest.skipIf(shutil.which('node') is None, 'this computer has no node on PATH')
    def test_the_real_report_runs_under_the_node_of_this_computer(self):
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder)
            copy = directory / 'pi-report.mjs'
            copy.write_text(REPORT.read_text())
            finished = subprocess.run(
                [sys.executable, str(SCRIPT), '-e', str(copy)],
                input='start\nask\nanswer\nstop\nquit\n', text=True, capture_output=True,
                check=False, env=environment(directory, recorder(directory)), timeout=120)
            self.assertEqual(finished.returncode, 0, finished.stderr)
            self.assertIn('the report runs under node', finished.stdout)
            wait_for_calls(directory, 5)
            events = documents(directory)
            self.assertEqual(sorted(event['hook_event_name'] for event in events),
                             sorted(['UserPromptSubmit', 'PermissionRequest', 'PostToolUse', 'Stop',
                                     'SessionEnd']))
            submits = [event['prompt_id'] for event in events
                       if event['hook_event_name'] == 'UserPromptSubmit']
            stops = [event['prompt_id'] for event in events if event['hook_event_name'] == 'Stop']
            self.assertEqual(len(submits), 1, 'one busy span of this run')
            self.assertEqual(stops, submits, 'the stop closes the turn that it opened')
            self.assertTrue(HARNESS.is_file(), 'the harness of the report is beside the stand-in')


if __name__ == '__main__':
    unittest.main()
