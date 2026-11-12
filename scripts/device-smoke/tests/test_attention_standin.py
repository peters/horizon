#!/usr/bin/env python3
"""Self test for agent_attention_standin.py, with a recorder as the hook entry.

The repository runs it with the other device smoke tests:

    python3 -B -m unittest discover -s scripts/device-smoke/tests -v

Horizon gives the panel of an agent the identity of its launch, so the test
gives that environment to the stand-in and proves:

  * every argument of a Claude panel is accepted and ignored, and the session
    identity of `--session-id` or `--resume` reaches the events;
  * each typed word sends one normalized event to
    `<HORIZON_WORK_EXECUTABLE> --agent-work-hook` on standard input, in the
    environment of the launch;
  * a prompt and a tool content never reach an event, and only a stop that says
    that background work runs carries the count;
  * the turn identity follows `start`, so a stop names the turn that started;
  * a line that names no event says so and sends nothing;
  * without the work variables it prints one line and still answers `quit`.

Standard library only. Nothing is written outside the folder of a test.
"""
from __future__ import annotations

import json
import os
import stat
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCRIPT = HERE.parent / 'agent_attention_standin.py'
MODULE = 'agent_attention_standin'
MODULE_PREFIX = 'attention-stand-in: '
WORK_VARIABLES = (
    'HORIZON_WORK_ROOT',
    'HORIZON_WORK_PANEL',
    'HORIZON_WORK_OWNER',
    'HORIZON_WORK_KIND',
    'HORIZON_WORK_EXECUTABLE',
)
# The arguments that Horizon inserts between the program of a Claude panel and the
# arguments of the panel entry, with an argument that no release knows.
HORIZON_ARGUMENTS = ['--plugin-dir', 'X', '--session-id', 'Y', '--plugin-dir', 'Z', '--unknown', 'W']
# The recorder takes the place of `<candidate> --agent-work-hook`: it keeps the
# arguments, the environment and the event that the stand-in sent it.
RECORDER = '''#!/usr/bin/env python3
import json, os, sys
folder = os.path.dirname(os.path.abspath(__file__))
record = {'arguments': sys.argv[1:],
          'environment': {name: os.environ.get(name) for name in NAMES},
          'input': sys.stdin.read()}
with open(os.path.join(folder, 'calls.jsonl'), 'a', encoding='utf-8') as calls:
    calls.write(json.dumps(record) + '\\n')
'''


def load():
    """Import the stand-in without leaving a __pycache__ in the fixture folder."""
    sys.dont_write_bytecode = True
    if MODULE not in sys.modules:
        sys.path.insert(0, str(SCRIPT.parent))
        import agent_attention_standin  # noqa: F401  (imported for its functions)

    return sys.modules[MODULE]


def recorder(directory):
    """A small program that records one hook invocation of the stand-in."""
    path = directory / 'recorder'
    path.write_text(RECORDER.replace('NAMES', repr(list(WORK_VARIABLES))))
    path.chmod(path.stat().st_mode | stat.S_IXUSR)
    return path


def environment(directory, executable):
    """The environment of one launch of the panel, and the recorder as its hook entry."""
    values = dict(os.environ)
    values.update({
        'HORIZON_WORK_ROOT': str(directory / 'home'),
        'HORIZON_WORK_PANEL': 'panel-2',
        'HORIZON_WORK_OWNER': 'launch-owner',
        'HORIZON_WORK_KIND': 'claude',
        'HORIZON_WORK_EXECUTABLE': str(executable),
    })
    return values


def calls(directory):
    path = directory / 'calls.jsonl'
    if not path.is_file():
        return []
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]


def events(directory):
    return [json.loads(call['input']) for call in calls(directory)]


class StandInTests(unittest.TestCase):

    def test_each_typed_word_sends_its_event_to_the_installed_hook_entry(self):
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder)
            executable = recorder(directory)
            lines = ['start', 'ask', 'answer', 'stop', 'background', 'start', 'failed', 'stop', 'quit']
            finished = subprocess.run(
                [sys.executable, str(SCRIPT), *HORIZON_ARGUMENTS],
                input=''.join(f'{line}\n' for line in lines), text=True, capture_output=True, check=False,
                env=environment(directory, executable), timeout=120)
            self.assertEqual(finished.returncode, 0, finished.stderr)
            self.assertEqual([event['hook_event_name'] for event in events(directory)],
                             ['UserPromptSubmit', 'PermissionRequest', 'PostToolUse', 'Stop', 'Stop',
                              'UserPromptSubmit', 'StopFailure', 'Stop'])
            # The launch names the conversation, and every event keeps it.
            self.assertEqual([event['session_id'] for event in events(directory)], ['Y'] * 8)
            self.assertEqual([event['prompt_id'] for event in events(directory)],
                             ['turn-1'] * 4 + ['turn-1'] + ['turn-2', 'turn-2', 'turn-2'])
            for call in calls(directory):
                self.assertEqual(call['arguments'], ['--agent-work-hook'])
                self.assertEqual(call['environment'], {
                    name: environment(directory, executable)[name] for name in WORK_VARIABLES})

    def test_the_event_paths_of_the_launch_are_absolute(self):
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder)
            finished = subprocess.run(
                [sys.executable, str(SCRIPT), '--resume', 'old-session'],
                input='start\nquit\n', text=True, capture_output=True, check=False,
                env=environment(directory, recorder(directory)), timeout=120)
            self.assertEqual(finished.returncode, 0, finished.stderr)
            event = events(directory)[0]
            self.assertTrue(os.path.isabs(event['cwd']))
            self.assertTrue(os.path.isabs(event['transcript_path']))
            self.assertEqual(event['session_id'], 'old-session', 'a resumed conversation is the same field')

    def test_an_event_carries_no_prompt_and_no_tool_content(self):
        stand_in = load()
        for word in stand_in.EVENTS:
            event = stand_in.event_document(word, 'turn-1', 'session')
            self.assertNotIn('prompt', event)
            self.assertNotIn('content', event)
            self.assertNotIn('tool_input', event)
            self.assertLessEqual(len(json.dumps(event)), 512, 'an event stays small')
            if word == 'background':
                self.assertEqual(event['background_tasks'], 2)
            else:
                self.assertNotIn('background_tasks', event)

    def test_a_line_that_names_no_event_says_so_and_sends_nothing(self):
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder)
            finished = subprocess.run(
                [sys.executable, str(SCRIPT)], input='sing\nquit\n', text=True, capture_output=True,
                check=False, env=environment(directory, recorder(directory)), timeout=120)
            self.assertEqual(finished.returncode, 0, finished.stderr)
            self.assertIn('unknown event', finished.stdout)
            self.assertEqual(calls(directory), [])

    def test_without_the_work_variables_it_says_one_line_and_still_answers_quit(self):
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder)
            values = {name: value for name, value in os.environ.items() if name not in WORK_VARIABLES}
            values['HORIZON_WORK_EXECUTABLE'] = ''
            finished = subprocess.run(
                [sys.executable, str(SCRIPT), *HORIZON_ARGUMENTS], input='start\nquit\n', text=True,
                capture_output=True, check=False, env=values, timeout=120)
            self.assertEqual(finished.returncode, 0, finished.stderr)
            first = finished.stdout.splitlines()[0]
            self.assertIn('HORIZON_WORK_ROOT', first, 'the line names the setting that is missing')
            self.assertIn('work_resume', first, 'the line names the setting of the fixture config')
            self.assertIn('no event is sent', finished.stdout, 'the panel stays alive and answers')
            self.assertEqual(calls(directory), [])

    def test_a_typed_word_is_read_while_the_timer_runs(self):
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder)
            values = environment(directory, recorder(directory))
            child = subprocess.Popen(
                [sys.executable, str(SCRIPT), '--timer', '60'], text=True, bufsize=1,
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=values)
            try:
                # The timer of sixty seconds sends its first event and then
                # waits, so a later event can only come from the person.
                deadline = time.monotonic() + 15
                answers = []
                while time.monotonic() < deadline and len(events(directory)) < 1:
                    time.sleep(0.05)
                child.stdin.write('ask\n')
                child.stdin.flush()
                while time.monotonic() < deadline and len(events(directory)) < 2:
                    time.sleep(0.05)
                child.stdin.write('quit\n')
                child.stdin.flush()
                answers = child.communicate(timeout=20)[0].splitlines()
            finally:
                if child.poll() is None:
                    child.kill()
                    child.wait()
            names = [event['hook_event_name'] for event in events(directory)]
            self.assertEqual(names, ['UserPromptSubmit', 'PermissionRequest'], 'the typed word reaches the hook')
            self.assertEqual(events(directory)[1]['prompt_id'], 'turn-1', 'the typed event names the open turn')
            self.assertIn(f'{MODULE_PREFIX}ask: PermissionRequest', answers)
            self.assertEqual(child.returncode, 0)

    def test_the_sequence_of_the_timer_names_only_known_events(self):
        stand_in = load()
        self.assertEqual(stand_in.TIMER_SEQUENCE[0], 'start')
        self.assertIn('ask', stand_in.TIMER_SEQUENCE)
        self.assertIn('background', stand_in.TIMER_SEQUENCE)
        self.assertEqual(set(stand_in.TIMER_SEQUENCE) - set(stand_in.EVENTS), set())

    def test_the_arguments_of_horizon_are_ignored_and_the_two_of_the_stand_in_are_read(self):
        stand_in = load()
        options = stand_in.parse_arguments([*HORIZON_ARGUMENTS, '--continue', '-c', 'text'])
        self.assertIsNone(options.config)
        self.assertIsNone(options.timer)
        options = stand_in.parse_arguments([*HORIZON_ARGUMENTS, '--timer', '3', '--config', '/tmp/a.json'])
        self.assertEqual(options.timer, '3')
        self.assertEqual(options.config, '/tmp/a.json')
        self.assertEqual(stand_in.parse_arguments(['--timer=2']).timer, '2')


if __name__ == '__main__':
    unittest.main()
