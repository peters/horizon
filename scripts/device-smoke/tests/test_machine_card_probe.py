#!/usr/bin/env python3
"""Self test for machine_card_probe.py, with a fake relay in this same file.

The repository runs it with the other device smoke tests:

    python3 -B -m unittest discover -s scripts/device-smoke/tests -v

The probe is the tool that a procedure uses to prove that every call of an agent of
a machine is answered through a cut of the link: it starts the relay program of the
skill and calls the card socket through it again and again. This test runs the probe
against a relay of this file, which answers `initialize` and every `tools/call`, and
proves:

  * every call gets an answer, so the probe ends with status 0 and says so;
  * the calls are `block_panel list` calls of the one machine that the probe names;
  * a call that the relay answers with the error of a lost link is no pass: the probe
    names that call and ends with status 1.

A run of this file with one argument is that relay.

Standard library only. Nothing is written outside the folder of a test.
"""
from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
PROBE = HERE.parent / 'machine_card_probe.py'
THIS_FILE = Path(__file__).resolve()

# The error that the relay program of the machines skill answers a call with when the
# link to Horizon ended.
LOST = 'The link to Horizon was lost. Call again.'


def serve_fake_relay() -> int:
    """Answers the calls of one probe, and names them in the file of `FAKE_RELAY_LOG`.

    `FAKE_RELAY_ERROR` names the number of the `tools/call` call that ends with the
    error of a lost link instead of an answer.
    """
    log_path = os.environ.get('FAKE_RELAY_LOG')
    failing = int(os.environ.get('FAKE_RELAY_ERROR', '0') or 0)
    calls = 0
    log = open(log_path, 'a', encoding='utf-8') if log_path else None
    try:
        for line in sys.stdin:
            message = json.loads(line)
            if log:
                log.write(line.rstrip('\n') + '\n')
                log.flush()
            if 'id' not in message:
                continue
            if message.get('method') == 'initialize':
                print(json.dumps({'jsonrpc': '2.0', 'id': message['id'], 'result': {
                    'protocolVersion': '2025-06-18', 'capabilities': {},
                    'serverInfo': {'name': 'fake-relay', 'version': '1'}}}), flush=True)
                continue
            calls += 1
            if calls == failing:
                print(json.dumps({'jsonrpc': '2.0', 'id': message['id'],
                                  'error': {'code': -32000, 'message': LOST}}), flush=True)
                continue
            print(json.dumps({'jsonrpc': '2.0', 'id': message['id'], 'result': {
                'structuredContent': {'status': 'listed'}}}), flush=True)
    finally:
        if log:
            log.close()
    return 0


class ProbeTest(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory(prefix='hz-card-probe-')
        self.addCleanup(self.folder.cleanup)
        self.root = Path(self.folder.name)

    def run_probe(self, *words: str, error: int = 0) -> tuple[subprocess.CompletedProcess, list]:
        """One run of the probe against the relay of this file, and its calls."""
        log = self.root / f'relay-calls-{error}.jsonl'
        env = dict(os.environ, FAKE_RELAY_LOG=str(log),
                   FAKE_RELAY_ERROR=str(error) if error else '')
        answer = subprocess.run(
            [sys.executable, '-u', str(PROBE), '--machine', 'build-1',
             '--relay', str(THIS_FILE), *words],
            capture_output=True, text=True, timeout=120, env=env)
        calls = [json.loads(line) for line in log.read_text(encoding='utf-8').splitlines()]
        return answer, [message['params'] for message in calls
                        if message.get('method') == 'tools/call']

    def test_the_probe_reads_one_answer_for_every_call(self):
        answer, calls = self.run_probe('--calls', '3', '--pause', '0.1')
        self.assertEqual(answer.returncode, 0, answer.stdout + answer.stderr)
        self.assertIn('3 of 3 calls are answered', answer.stdout)
        self.assertIn('status 0', answer.stdout)
        self.assertEqual([call['name'] for call in calls], ['block_panel'] * 3, calls)
        self.assertEqual([call['arguments']['operation'] for call in calls], ['list'] * 3, calls)

    def test_a_call_that_an_error_answers_is_no_pass(self):
        answer, calls = self.run_probe('--calls', '3', '--pause', '0.1', error=2)
        self.assertEqual(answer.returncode, 1, answer.stdout + answer.stderr)
        self.assertIn('call 3 was answered with an error', answer.stdout)
        self.assertIn(LOST, answer.stdout)
        self.assertIn('2 of 3 calls are answered; status 1', answer.stdout)
        self.assertEqual(len(calls), 3, 'the probe calls again after the error')


if __name__ == '__main__':
    if len(sys.argv) == 2:
        # The probe of this test runs this file as its relay program.
        sys.exit(serve_fake_relay())
    unittest.main()
