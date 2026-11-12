#!/usr/bin/env python3
"""Self test for frame_source_agent.py, with a tiny fake MCP server in this same file.

The repository runs it with the other device smoke tests:

    python3 -B -m unittest discover -s scripts/device-smoke/tests -v

It starts `frame_source_agent.py` as a child, gives it this file as its MCP server
(`<python> test_frame_source_agent.py --serve-fake`), types one word for each
operation on the child's standard input, and proves:

  * the initialize handshake and notifications/initialized;
  * one `device_panel` call for each word, with the arguments of the operation, and
    no `panel_id` of a viewer that the run did not create;
  * `create <name> [target]` passes a name and a target and keeps the id of the
    viewer, which the real answer holds inside `panels[0]`;
  * `watch n` inspects the viewer n times, one second apart;
  * `hide` and `show` change the visibility of the last viewer, and `close` ends it;
  * a refusal of the host is one line with its code and its message;
  * every line the agent prints is safe for the state scrape of the host.

Standard library only. Nothing is written outside the folder of this file.
"""
from __future__ import annotations

import sys

sys.dont_write_bytecode = True

from pathlib import Path
import json
import os
import subprocess
import tempfile
import threading
import time
import unittest

HERE = str(Path(__file__).resolve().parent)
AGENT = str(Path(HERE).parent / 'frame_source_agent.py')
THIS_FILE = str(Path(__file__).resolve())

# The words that Horizon reads as a working agent (crates/horizon-core/src/agents.rs).
WORKING_KEYWORDS = ('Working', 'Thinking', 'Compacting', 'Retrying', 'Reusing', 'Summarizing',
                    'Context overflow')
WORKING_HINTS = ('to interrupt', 'to stop', 'to cancel')
SPINNER_GLYPHS = ('✻', '✽', '✶', '·', '✢', '✳', '✏', '⏳')


class FakeHost:
    """A newline-delimited JSON-RPC MCP server that answers `device_panel`.

    It records each tool call as one JSON line of the log, so the test reads the
    arguments of the operation that each word made. `create` answers one panel id,
    `inspect` a connected viewer with a rising frame sequence, and every operation of
    another panel id is refused with `panel_unavailable`.
    """

    def __init__(self, log_path: str, mode: str = 'full'):
        self.log = open(log_path, 'a', buffering=1, encoding='utf-8')
        self.mode = mode
        self.panel_id = None
        self.inspections = 0

    def record(self, entry: dict) -> None:
        self.log.write(json.dumps(entry, sort_keys=True) + '\n')

    def answer(self, arguments: dict) -> dict:
        """One answer of `device_panel`, with the shape that Horizon sends.

        The shapes are the ones of crates/horizon-browser-control/src/manifest/
        device.rs: `Outcome` is tagged by `status`, the operations that name a
        viewer answer `Outcome::Panels { panels: Vec<PanelState> }`, `close`
        answers `Outcome::Closed { panel_id }`, and a refusal answers
        `Outcome::Failed { code, message }`.
        """
        operation = arguments.get('operation')
        self.record({'operation': operation, 'arguments': arguments})
        if self.mode == 'refuse' and operation == 'close':
            return {'status': 'failed', 'code': 'not_owner',
                    'message': 'Only the owning agent may mutate this Device panel'}
        if operation == 'create':
            request = arguments.get('frame_source') or {}
            self.panel_id = 'd1'
            return {'status': 'panels', 'panels': [self.panel(
                connection='connecting', visible=True, image_received=False,
                image_displayed=False, frame_sequence=0, source=request.get('name'),
                target=request.get('target'))]}
        if operation == 'list':
            return {'status': 'panels', 'panels': [self.panel(connection='connected')]}
        if arguments.get('panel_id') != self.panel_id:
            return {'status': 'failed', 'code': 'panel_unavailable',
                    'message': 'Device panel is unavailable in this workspace'}
        if operation == 'inspect':
            self.inspections += 1
            return {'status': 'panels', 'panels': [self.panel(
                connection='connected', image_received=True, image_displayed=True,
                frame_sequence=self.inspections)]}
        if operation == 'visibility':
            return {'status': 'panels', 'panels': [self.panel(
                connection='connected', visible=arguments.get('visible'))]}
        if operation == 'close':
            return {'status': 'closed', 'panel_id': self.panel_id}
        return {'status': 'failed', 'code': 'invalid_operation', 'message': operation}

    def panel(self, connection='connected', visible=True, image_received=True,
              image_displayed=True, frame_sequence=1, source=None, target=None) -> dict:
        """One `PanelState` of the real answer.

        Its members are the fields of `PanelState` in
        crates/horizon-browser-control/src/manifest/device.rs, flattened with the
        `ImageEvidence` members and no optional member that the candidate omits.
        """
        panel = {'panel_id': self.panel_id, 'kind': 'frame_source',
                 'endpoint': source or 'standin',
                 'server': {'name': source, 'desktop_size': [640, 360]},
                 'visible': visible, 'owned_by_caller': True,
                 'connection': connection, 'connection_error': None,
                 'image_received': image_received, 'image_displayed': image_displayed,
                 'frame_sequence': frame_sequence,
                 'received_frame_sequence': frame_sequence,
                 'frames_dropped': 0, 'age_ms': 40}
        if target is not None:
            panel['target'] = target
        return panel

    def serve(self) -> int:
        for raw in sys.stdin.buffer:
            line = raw.strip()
            if not line:
                continue
            message = json.loads(line.decode('utf-8'))
            method = message.get('method')
            if method == 'initialize':
                self.reply(message, {'protocolVersion': '2024-11-05', 'capabilities': {},
                                     'serverInfo': {'name': 'fake', 'version': '1'}})
            elif method == 'notifications/initialized':
                self.record({'notification': method})
            elif method == 'tools/call':
                params = message.get('params') or {}
                if params.get('name') != 'device_panel':
                    self.reply(message, {'isError': True, 'content': [
                        {'type': 'text', 'text': 'no such tool'}]})
                    continue
                self.reply(message, {'content': [{'type': 'text', 'text': 'ok'}],
                                     'structuredContent': self.answer(params.get('arguments') or {})})
            elif 'id' in message:
                self.reply(message, {'error': {'code': -32601, 'message': method}})
        return 0

    def reply(self, message: dict, result: dict) -> None:
        answer = {'jsonrpc': '2.0', 'id': message.get('id'), 'result': result}
        sys.stdout.write(json.dumps(answer) + '\n')
        sys.stdout.flush()


class FrameSourceAgentTests(unittest.TestCase):
    """The words of the scripted client, against a host that records its calls."""

    def start(self, mode: str = 'full'):
        self.mode = mode
        self.folder = tempfile.TemporaryDirectory()
        self.log_path = os.path.join(self.folder.name, 'calls.jsonl')
        command = f'{sys.executable} -B {THIS_FILE} --serve-fake'
        environment = dict(os.environ)
        # The fake host writes the calls of this test into the log that it reads.
        environment['FAKE_MCP_LOG'] = self.log_path
        environment['FAKE_MCP_MODE'] = self.mode
        self.child = subprocess.Popen(
            [sys.executable, '-B', AGENT, '--mcp-command', command],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            bufsize=0, text=True, env=environment)
        self.lines = []
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()
        self.read_until('ready')

    def _read(self) -> None:
        for line in self.child.stdout:
            self.lines.append(line.rstrip('\n'))

    def read_until(self, word: str, timeout: float = 20.0) -> str:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            for line in self.lines:
                if word in line:
                    self.lines.remove(line)
                    return line
            time.sleep(0.02)
        self.fail(f'the client printed no line with {word!r}: {self.lines!r}')

    def type(self, word: str) -> str:
        self.child.stdin.write(word + '\n')
        self.child.stdin.flush()
        return self.read_until('->')

    def calls(self) -> list[dict]:
        path = Path(self.log_path)
        if not path.exists():
            return []
        return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]

    def operations(self) -> list[dict]:
        return [call for call in self.calls() if 'operation' in call]

    def tearDown(self):
        if getattr(self, 'child', None) is not None:
            self.child.stdin.close()
            try:
                self.child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.child.kill()
                self.child.wait(timeout=10)
        if getattr(self, 'child', None) is not None:
            for stream in (self.child.stdout, self.child.stderr):
                if stream is not None:
                    stream.close()
        if getattr(self, 'folder', None) is not None:
            self.folder.cleanup()

    def test_each_word_is_one_operation_of_the_tool(self):
        self.start()
        created = self.type('create standin lab:1')
        self.assertIn('"panel_id": "d1"', created)
        inspected = self.type('inspect')
        self.assertIn('"frame_sequence": 1', inspected)
        self.assertIn('"desktop_size": [640, 360]', inspected)
        self.assertIn('"age_ms": 40', inspected)
        self.type('hide')
        self.type('show')
        self.type('close')
        self.type('list')
        operations = [call['operation'] for call in self.operations()]
        self.assertEqual(operations, ['create', 'inspect', 'visibility', 'visibility', 'close',
                                      'list'])
        create = self.operations()[0]['arguments']['frame_source']
        self.assertEqual(create, {'name': 'standin', 'target': 'lab:1'})
        self.assertEqual(self.operations()[2]['arguments']['visible'], False)
        self.assertEqual(self.operations()[3]['arguments']['visible'], True)
        self.assertEqual(self.operations()[1]['arguments']['panel_id'], 'd1')

    def test_a_word_of_a_run_without_a_viewer_calls_no_operation(self):
        self.start()
        line = self.type('inspect')
        self.assertIn('needs a viewer', line)
        self.assertEqual(self.operations(), [], 'no viewer, no operation of the tool')

    def test_the_id_of_the_viewer_is_the_one_inside_panels(self):
        # The real answer of a create names its viewer in `panels[0].panel_id`
        # (crates/horizon-browser-control/src/manifest/device.rs), so the client
        # must keep that member and every later word must name it.
        self.start()
        created = self.type('create standin')
        self.assertIn('"panel_id": "d1"', created)
        self.assertIn('"status": "panels"', created)
        self.assertIn('"connection": "connecting"', created)
        self.assertIn('"desktop_size": [640, 360]', created)
        inspected = self.type('inspect')
        self.assertNotIn('needs a viewer', inspected)
        self.assertIn('"image_displayed": true', inspected)
        self.assertEqual(self.operations()[-1]['arguments']['panel_id'], 'd1')

    def test_a_refusal_of_the_host_is_one_line_with_its_code(self):
        self.start(mode='refuse')
        created = self.type('create standin')
        self.assertIn('"kind": "frame_source"', created)
        refused = self.type('close')
        self.assertIn('"status": "failed"', refused)
        self.assertIn('"code": "not_owner"', refused)
        self.assertIn('Only the owning agent', refused)
        self.assertEqual(self.operations()[-1]['arguments']['panel_id'], 'd1',
                         'the refusal names the viewer of this run')

    def test_every_line_is_safe_for_the_state_scrape_of_the_host(self):
        self.start()
        self.type('create standin')
        self.type('inspect')
        self.type('watch 2')
        self.type('close')
        for line in self.lines + ['frame-source-agent: ready']:
            for word in WORKING_KEYWORDS:
                self.assertNotIn(word, line)
            for hint in WORKING_HINTS:
                self.assertNotIn(hint, line)
            for glyph in SPINNER_GLYPHS:
                self.assertNotIn(glyph, line)
            self.assertTrue(line.startswith('frame-source-agent: '), line)

    def test_watch_inspects_the_viewer_once_a_second(self):
        self.start()
        self.type('create standin')
        first = self.type('watch 2')
        second = self.read_until('->')
        self.assertIn('watch 1/2', first)
        self.assertIn('watch 2/2', second)
        self.assertIn('"frame_sequence": 1', first)
        self.assertIn('"frame_sequence": 2', second)
        self.assertEqual([call['operation'] for call in self.operations()],
                         ['create', 'inspect', 'inspect'])


if __name__ == '__main__':
    if '--serve-fake' in sys.argv:
        # The test names the log and the mode through the environment, so one
        # server per run writes the file that its own test reads.
        log = os.environ.get('FAKE_MCP_LOG')
        if not log:
            raise SystemExit('--serve-fake needs FAKE_MCP_LOG')
        sys.exit(FakeHost(log, os.environ.get('FAKE_MCP_MODE', 'full')).serve())
    unittest.main()
