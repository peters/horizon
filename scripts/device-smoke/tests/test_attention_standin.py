#!/usr/bin/env python3
"""Self test for agent_attention_standin.py, with a fake hook entry in this file.

The repository runs it with the other device smoke tests:

    python3 -B -m unittest discover -s scripts/device-smoke/tests -v

The fake entry records each event that the stand-in sends, so the test proves:

  * the work record and the health marker follow the shape that
    crates/horizon-core/src/agent_work/store.rs reads, at the encoded panel path;
  * each typed word sends one normalized event through the configured entry;
  * a prompt and a tool content never reach an event, and only a stop that says
    that background work runs carries the count;
  * the turn identity follows `start`, so a stop names the turn that started;
  * a line that names no event says so and sends nothing.

Standard library only. Nothing is written outside the folder of this file.
"""
from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCRIPT = HERE.parent / 'agent_attention_standin.py'
MODULE = 'agent_attention_standin'


def load():
    """Import the stand-in without leaving a __pycache__ in the fixture folder."""
    sys.dont_write_bytecode = True
    if MODULE not in sys.modules:
        sys.path.insert(0, str(SCRIPT.parent))
        import agent_attention_standin  # noqa: F401  (imported for its functions)

    return sys.modules[MODULE]


def document(directory):
    """The stand-in config of one test, with this file as the hook entry."""
    return {
        'root': str(directory / 'home'),
        'panel': 'panel-2',
        'owner': 'stand-in-owner',
        'kind': 'claude',
        'cwd': str(directory),
        'transcript_path': '',
        'config': str(directory / 'stand-in.json'),
        'exe': 'the-candidate',
        'hook': [sys.executable, str(Path(__file__).resolve()), '--record-hook', str(directory / 'events.jsonl')],
    }


def write_config(directory, config):
    path = directory / 'stand-in.json'
    path.write_text(json.dumps(config))
    return path


def recorded(directory):
    path = directory / 'events.jsonl'
    if not path.is_file():
        return []
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]


class StandInTests(unittest.TestCase):

    def test_the_record_and_the_health_marker_are_what_the_store_reads(self):
        stand_in = load()
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder)
            path = Path(stand_in.register(document(directory)))
            panel = '%' + ''.join(f'{byte:02x}' for byte in b'panel-2')
            self.assertEqual(path, directory / 'home' / 'agent-work' / f'{panel}.json')
            record = json.loads(path.read_text())
            self.assertEqual(record['version'], 1)
            self.assertEqual(record['panel_local_id'], 'panel-2')
            self.assertEqual(record['kind'], 'claude')
            self.assertEqual(record['owner_token'], 'stand-in-owner')
            self.assertEqual(record['ledger'], {}, 'no event touched the ledger yet')
            self.assertIsNone(record['handoff'])
            self.assertFalse(record['consumed'])
            marker = directory / 'home' / 'agent-work' / 'health' / panel / 'stand-in-owner'
            self.assertEqual(marker.read_text(), '1')
            self.assertEqual(marker.stat().st_mode & 0o777, 0o600)
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)

    def test_each_typed_word_sends_the_event_of_its_line(self):
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder)
            path = write_config(directory, document(directory))
            lines = ['start', 'ask', 'answer', 'stop', 'background', 'start', 'failed', 'stop', 'quit']
            finished = subprocess.run(
                [sys.executable, str(SCRIPT), '--config', str(path)],
                input=''.join(f'{line}\n' for line in lines), text=True, capture_output=True, check=False,
                timeout=120)
            self.assertEqual(finished.returncode, 0, finished.stderr)
            events = recorded(directory)
            self.assertEqual(
                [event['hook_event_name'] for event in events],
                ['UserPromptSubmit', 'PermissionRequest', 'PostToolUse', 'Stop', 'Stop', 'UserPromptSubmit',
                 'StopFailure', 'Stop'])
            self.assertEqual([event['session_id'] for event in events], ['stand-in-session'] * 8)
            # The turn identity follows `start`, and the first stop is the turn
            # that the second start leaves behind.
            self.assertEqual([event['prompt_id'] for event in events],
                             ['turn-1'] * 4 + ['turn-1'] + ['turn-2', 'turn-2', 'turn-2'])

    def test_an_event_carries_no_prompt_and_no_tool_content(self):
        stand_in = load()
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder)
            config = document(directory)
            for word in stand_in.EVENTS:
                event = stand_in.event_document(config, word, 'turn-1')
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
            path = write_config(directory, document(directory))
            finished = subprocess.run(
                [sys.executable, str(SCRIPT), '--config', str(path)], input='sing\nquit\n', text=True,
                capture_output=True, check=False, timeout=120)
            self.assertEqual(finished.returncode, 0, finished.stderr)
            self.assertIn('unknown event', finished.stdout)
            self.assertEqual(recorded(directory), [])

    def test_a_cycle_of_the_timer_sends_the_sequence_without_a_line(self):
        stand_in = load()
        self.assertEqual(stand_in.TIMER_SEQUENCE[0], 'start')
        self.assertIn('ask', stand_in.TIMER_SEQUENCE)
        self.assertIn('background', stand_in.TIMER_SEQUENCE)
        self.assertEqual(set(stand_in.TIMER_SEQUENCE) - set(stand_in.EVENTS), set())


if __name__ == '__main__':
    if '--record-hook' in sys.argv:
        # The fake entry point: append the event that the stand-in sent.
        index = sys.argv.index('--record-hook')
        with open(sys.argv[index + 1], 'a', encoding='utf-8') as events:
            events.write(sys.stdin.read().strip() + '\n')
        sys.exit(0)
    unittest.main()
