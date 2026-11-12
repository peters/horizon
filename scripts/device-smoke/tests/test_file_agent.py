#!/usr/bin/env python3
"""Self test for file_agent.py, with a small fake MCP server in this same file.

The repository runs it with the other device smoke tests:

    python3 -B -m unittest discover -s scripts/device-smoke/tests -v

It starts `file_agent.py` as a child, gives it this file as its MCP server
(`<python> test_file_agent.py --serve-fake`) and a command file and an answer
file under a temporary directory, and proves:

  * one line with a tool and its arguments runs that tool once, prints one line
    in the panel and appends `{"request": ..., "answer": ...}` to the answer file;
  * a line that names no `arguments` and no `timeout` calls the tool with an
    empty object and the default 30 seconds;
  * a refused tool is an answer and not a crash, and the next line still runs;
  * a tool that answers nothing within its `timeout` is an answer too;
  * a line that is not JSON, and a line without a `tool` name, are reported and
    skipped, and their text stays in the answer file;
  * a line counts only when its newline is in the file, so a half-written line
    runs when it is complete;
  * a file that a person writes again from its first line is read again, and a
    command that was already run does not run twice;
  * `{"quit": true}` ends the run before the lines below it, and the child exits 0;
  * the fixture makes the command file, the answer file and the panel of the file
    agent, and names the two files in its manifest.

Standard library only. The temporary directory of a test holds every file.
"""
from __future__ import annotations

import sys

# Importing file_agent.py must not leave a __pycache__ beside the programs.
sys.dont_write_bytecode = True

from pathlib import Path
import json
import os
import queue
import shlex
import subprocess
import tempfile
import threading
import time
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import file_agent  # noqa: E402

HERE = str(Path(__file__).resolve().parent)
AGENT = str(Path(file_agent.__file__).resolve())
THIS_FILE = str(Path(__file__).resolve())


# --------------------------------------------------------------------------
# The fake MCP server. `python3 test_file_agent.py --serve-fake`
# --------------------------------------------------------------------------

class FakeServer:
    """A newline-delimited JSON-RPC MCP server with three tools and one refusal.

    FAKE_MCP_LOG holds every request and its answer, so a test proves the
    arguments and the timeout that the agent sent.
    """

    def __init__(self, log_path):
        self.log = open(log_path, 'a', buffering=1, encoding='utf-8')

    def record(self, entry):
        self.log.write(json.dumps(entry, sort_keys=True) + '\n')

    def call(self, name, arguments):
        if name == 'machines':
            return {'status': 'machines', 'machines': [{'name': 'build-1'}]}
        if name == 'machine_set':
            return {'status': 'machine_set', 'name': arguments.get('name'), 'created': True}
        if name == 'refused':
            return {'status': 'failed', 'code': 'not_owner',
                    'message': 'another agent created this card'}
        if name == 'slow':
            time.sleep(0.5)
            return {'status': 'slow', 'slept': True}
        return {'status': 'failed', 'code': 'unknown_tool', 'message': name}

    def handle(self, message):
        method = message.get('method')
        request_id = message.get('id')
        if method == 'initialize':
            return {'jsonrpc': '2.0', 'id': request_id, 'result': {
                'protocolVersion': '2025-06-18',
                'capabilities': {'tools': {}},
                'serverInfo': {'name': 'horizon-browser', 'version': 'fake'}}}
        if method == 'tools/list':
            return {'jsonrpc': '2.0', 'id': request_id, 'result': {'tools': [
                {'name': 'machines', 'inputSchema': {'type': 'object'}},
                {'name': 'machine_set', 'inputSchema': {'type': 'object'}},
                {'name': 'refused', 'inputSchema': {'type': 'object'}},
                {'name': 'slow', 'inputSchema': {'type': 'object'}}]}}
        if method == 'tools/call':
            params = message.get('params') or {}
            payload = self.call(params.get('name'), params.get('arguments') or {})
            refused = payload.get('status') == 'failed'
            self.record({'method': 'tools/call', 'params': params, 'answer': payload})
            # A refused tool is one error result of the MCP protocol, as Horizon
            # answers a refused operation.
            return {'jsonrpc': '2.0', 'id': request_id, 'result': {
                'content': [{'type': 'text', 'text': json.dumps(payload)}],
                'structuredContent': payload,
                'isError': refused}}
        if request_id is None:
            return None
        return {'jsonrpc': '2.0', 'id': request_id,
                'error': {'code': -32601, 'message': f'no method {method}'}}

    def serve(self):
        for raw in iter(sys.stdin.buffer.readline, b''):
            line = raw.strip()
            if not line:
                continue
            reply = self.handle(json.loads(line))
            if reply is None:
                continue
            sys.stdout.write(json.dumps(reply) + '\n')
            sys.stdout.flush()
        return 0


# --------------------------------------------------------------------------
# The test driver.
# --------------------------------------------------------------------------

class AgentRun:
    """One file_agent.py child, its panel lines, and the fake server's log."""

    def __init__(self, test, folder):
        self.test = test
        self.folder = Path(folder)
        self.commands = self.folder / 'commands.jsonl'
        self.answers = self.folder / 'answers.jsonl'
        self.commands.write_text('')
        self.log_path = self.folder / 'fake-mcp.jsonl'
        command = f'{sys.executable} {shlex.quote(THIS_FILE)} --serve-fake'
        self.env = dict(os.environ)
        self.env.update({'FAKE_MCP_LOG': str(self.log_path)})
        self.proc = subprocess.Popen(
            [sys.executable, AGENT, '--mcp-command', command,
             '--commands', str(self.commands), '--answers', str(self.answers)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            env=self.env, cwd=HERE)
        self.lines = queue.Queue()
        threading.Thread(target=self._pump_stdout, daemon=True).start()
        self.stdout_text = ''

    def _pump_stdout(self):
        for raw in iter(self.proc.stdout.readline, b''):
            self.lines.put(raw.decode('utf-8', 'replace').rstrip('\r\n'))

    def write(self, text):
        """Append one command line, as a person's program does."""
        with self.commands.open('a', encoding='utf-8') as handle:
            handle.write(text)
            handle.flush()

    def entries(self):
        if not self.answers.exists():
            return []
        with self.answers.open(encoding='utf-8') as handle:
            return [json.loads(line) for line in handle if line.strip()]

    def calls(self):
        if not self.log_path.exists():
            return []
        with self.log_path.open(encoding='utf-8') as handle:
            entries = [json.loads(line) for line in handle if line.strip()]
        return [entry for entry in entries if entry.get('method') == 'tools/call']

    def wait_for(self, predicate, timeout=15.0):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if predicate():
                return True
            if self.proc.poll() is not None:
                return predicate()
            time.sleep(0.05)
        return predicate()

    def wait_for_entries(self, count, timeout=15.0):
        return self.wait_for(lambda: len(self.entries()) >= count, timeout)

    def wait_for_line(self, text, timeout=15.0):
        """Wait until the panel holds one line with `text` in it."""
        self.snapshot()
        return self.wait_for(lambda: text in self.snapshot(), timeout)

    def snapshot(self):
        drained = []
        while True:
            try:
                drained.append(self.lines.get_nowait())
            except queue.Empty:
                break
        self.stdout_text += ''.join(f'{line}\n' for line in drained)
        return self.stdout_text

    def finish(self, timeout=20.0):
        try:
            code = self.proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait()
            self.test.fail(f'the file agent did not exit; panel so far: {self.snapshot()}')
        errors = self.proc.stderr.read().decode('utf-8', 'replace')
        for stream in (self.proc.stdin, self.proc.stdout, self.proc.stderr):
            try:
                if stream is not None:
                    stream.close()
            except (OSError, ValueError):
                pass
        return code, self.snapshot(), errors


class FileAgentTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix='file-agent-')
        self.addCleanup(self.directory.cleanup)
        self.run = AgentRun(self, self.directory.name)
        self.addCleanup(self._stop)

    def _stop(self):
        if self.run.proc.poll() is None:
            self.run.proc.terminate()
            try:
                self.run.proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.run.proc.kill()
                self.run.proc.wait()
        self.run.snapshot()

    def test_one_line_runs_its_tool_and_keeps_the_answer(self):
        self.run.write('{"tool": "machine_set", "arguments": {"name": "build-1", "ssh": {"host": "127.0.0.1"}}, "timeout": 5}\n')
        self.assertTrue(self.run.wait_for_entries(1), 'one answer entry arrived')
        entry = self.run.entries()[0]
        self.assertEqual(entry['request']['tool'], 'machine_set')
        self.assertEqual(entry['request']['arguments'], {'name': 'build-1', 'ssh': {'host': '127.0.0.1'}})
        self.assertEqual(entry['answer'], {'status': 'machine_set', 'name': 'build-1', 'created': True})
        calls = self.run.calls()
        self.assertEqual(len(calls), 1)
        self.assertEqual(calls[0]['params']['arguments']['name'], 'build-1')
        self.assertTrue(self.run.wait_for_line('file-agent: machine_set -> '),
                        f'the panel holds one line with the answer: {self.run.snapshot()}')

    def test_a_line_without_arguments_and_timeout_uses_the_defaults(self):
        self.run.write('{"tool": "machines"}\n')
        self.assertTrue(self.run.wait_for_entries(1))
        entry = self.run.entries()[0]
        self.assertEqual(entry['request']['arguments'], {})
        self.assertEqual(entry['request']['timeout'], file_agent.DEFAULT_TIMEOUT_SECONDS)
        self.assertEqual(entry['answer']['machines'], [{'name': 'build-1'}])

    def test_a_refused_tool_is_an_answer_and_the_next_line_still_runs(self):
        self.run.write('{"tool": "refused"}\n')
        self.assertTrue(self.run.wait_for_entries(1))
        self.run.write('{"tool": "machines"}\n')
        self.assertTrue(self.run.wait_for_entries(2))
        refused, answered = self.run.entries()
        self.assertEqual(refused['answer']['status'], 'refused')
        self.assertIn('not_owner', refused['answer']['error'])
        self.assertEqual(answered['answer']['status'], 'machines')
        self.assertIsNone(self.run.proc.poll(), 'a refusal does not end the program')
        self.assertIn('not_owner', self.run.snapshot())

    def test_a_tool_that_answers_too_late_is_an_answer(self):
        self.run.write('{"tool": "slow", "timeout": 0.2}\n')
        self.assertTrue(self.run.wait_for_entries(1))
        answer = self.run.entries()[0]['answer']
        self.assertEqual(answer['status'], 'refused')
        self.assertIn('answer', answer['error'], 'the reason names the missing answer')

    def test_a_line_that_is_not_json_is_reported_and_skipped(self):
        self.run.write('this is not JSON\n')
        self.assertTrue(self.run.wait_for_entries(1))
        self.run.write('{"arguments": {}}\n')
        self.assertTrue(self.run.wait_for_entries(2))
        self.run.write('{"tool": "machines"}\n')
        self.assertTrue(self.run.wait_for_entries(3))
        not_json, no_tool, answered = self.run.entries()
        self.assertEqual(not_json['answer']['status'], 'skipped')
        self.assertIn('not JSON', not_json['answer']['error'])
        self.assertEqual(not_json['request'], {'line': 'this is not JSON'},
                         'the skipped line stays in the answer file as evidence')
        self.assertEqual(no_tool['answer']['status'], 'skipped')
        self.assertIn('"tool"', no_tool['answer']['error'])
        self.assertEqual(answered['answer']['status'], 'machines')
        self.assertTrue(self.run.wait_for_line('skipped one line'))
        shown = self.run.snapshot()
        self.assertIn('not JSON', shown, 'the panel names the reason of the skipped line')
        self.assertEqual(len(self.run.calls()), 1, 'a skipped line calls no tool')

    def test_a_line_runs_when_its_newline_is_there(self):
        self.run.write('{"tool": "machines"')
        time.sleep(1.5)
        self.assertEqual(self.run.entries(), [],
                         'a half-written line is not a command yet')
        self.run.write('}\n')
        self.assertTrue(self.run.wait_for_entries(1))
        self.assertEqual(self.run.entries()[0]['answer']['status'], 'machines')

    def test_a_command_runs_once_and_a_file_written_again_is_read_again(self):
        self.run.write('{"tool": "machines"}\n')
        self.assertTrue(self.run.wait_for_entries(1))
        time.sleep(1.5)
        self.assertEqual(len(self.run.entries()), 1, 'an old line does not run twice')
        self.run.commands.write_text('{"tool": "machine_set", "arguments": {"name": "build-2"}}\n')
        self.assertTrue(self.run.wait_for_entries(2))
        self.assertEqual(self.run.entries()[1]['answer']['name'], 'build-2')
        self.assertEqual(len(self.run.calls()), 2)

    def test_quit_ends_the_run_before_the_lines_below_it(self):
        self.run.write('{"quit": true}\n{"tool": "machines"}\n')
        code, shown, errors = self.run.finish()
        self.assertEqual(code, 0, errors)
        self.assertIn('file-agent: quit', shown)
        self.assertEqual(self.run.calls(), [], 'a line below quit runs nothing')


class FileAgentPanelTests(unittest.TestCase):
    def test_the_fixture_makes_the_two_files_and_the_panel_of_the_agent(self):
        import serve
        with tempfile.TemporaryDirectory(prefix='file-agent-panel-') as folder:
            state = Path(folder)
            commands, answers = serve.file_agent_files(state)
            self.assertEqual(commands, state / 'data' / serve.FILE_AGENT_FOLDER / serve.FILE_AGENT_COMMANDS)
            self.assertEqual(answers, state / 'data' / serve.FILE_AGENT_FOLDER / serve.FILE_AGENT_ANSWERS)
            self.assertTrue(commands.is_file())
            self.assertEqual(commands.read_text(), '', 'a run reads no command of an earlier run')
            panel = serve.file_agent_panel(Path('/opt/horizon'), state, commands, answers)
            self.assertEqual(panel['name'], 'File agent')
            self.assertEqual(panel['kind'], 'claude', 'an agent panel gives the tools an identity')
            self.assertEqual(panel['args'], [])
            launcher = Path(panel['command'])
            self.assertEqual(launcher.parent, state / 'data' / serve.FILE_AGENT_FOLDER)
            self.assertTrue(launcher.is_file(), 'the panel runs a copy inside the state folder')
            script = launcher.read_text()
            self.assertIn(serve.FILE_AGENT_PROGRAM, script)
            self.assertIn(f'--commands {commands}', script)
            self.assertIn(f'--answers {answers}', script)
            self.assertIn('/opt/horizon --browser-mcp', script)
            self.assertIn('"$@"', script, 'Horizon launch arguments reach the script')
            # The agent imports the MCP client of the card agent, so that copy lies
            # beside it: the sandbox holds no checkout.
            self.assertTrue((launcher.parent / serve.WORKTREE_CARD_PROGRAM).is_file())
            self.assertEqual(serve.file_agent_manifest(commands, answers),
                             {'commands': str(commands), 'answers': str(answers)})

    def test_a_panels_entry_that_names_the_program_gets_both_copies(self):
        # The agent imports the MCP client of the card agent, so a copy of the agent
        # is not usable without the copy of the card agent beside it.
        import serve
        with tempfile.TemporaryDirectory(prefix='file-agent-panels-') as folder:
            state = Path(folder)
            (state / 'data').mkdir()
            program = serve.FIXTURE_FOLDER / serve.FILE_AGENT_PROGRAM
            panels = serve.panel_programs_in_state(
                [{'name': 'File agent', 'kind': 'claude', 'command': '/bin/sh',
                  'args': ['-c', f'exec /usr/bin/python3 -u {program} --mcp-command x']}], state)
            self.assertNotIn(str(program), panels[0]['args'][1], 'the checkout path is replaced')
            folder = state / 'data' / serve.PANEL_PROGRAM_FOLDER
            self.assertTrue((folder / serve.FILE_AGENT_PROGRAM).is_file())
            self.assertTrue((folder / serve.WORKTREE_CARD_PROGRAM).is_file(),
                            'the copy of the imported program lies beside it')

    def test_the_option_writes_settings_so_a_plain_start_refuses_it(self):
        import serve
        self.assertIn(('file_agent', '--file-agent'), serve.PLAIN_START_SETTINGS_OPTIONS)
        args = serve.arguments().parse_args(['--horizon', '/opt/horizon', '--state', '/tmp/x',
                                             '--file-agent'])
        self.assertTrue(args.file_agent)
        self.assertEqual(serve.plain_start_conflicts(args), ['--file-agent'])


if __name__ == '__main__':
    if '--serve-fake' in sys.argv[1:]:
        log = os.environ.get('FAKE_MCP_LOG')
        if not log:
            print('FAKE_MCP_LOG is required', file=sys.stderr)
            sys.exit(2)
        sys.exit(FakeServer(log).serve())
    unittest.main(verbosity=2)
