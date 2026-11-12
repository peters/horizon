#!/usr/bin/env python3
"""A scripted client of Horizon's `device_panel` tool, for the frame source procedure.

The procedure of the frame source viewer needs a viewer that the candidate in the
fixture opens itself. The public tool belongs to the Horizon of the person who runs
the test, not to the candidate inside the fixture, so this script is the program of
an agent panel of the fixture: it starts the MCP server of the candidate as a child,
speaks newline-delimited JSON-RPC 2.0 over the child's standard input and output, and
calls the one tool `device_panel`.

A person types one word on its standard input, one operation per line:

    create [name] [target]  create a viewer of that frame source program
    inspect                 inspect the viewer of this run
    watch [n]               inspect once a second, n times (default 3)
    hide                    hide the viewer
    show                    show the viewer
    close                   close the viewer
    list                    list the viewers of this workspace

Each answer is one line: the word, and the evidence members of the viewer that the
answer names. An operation that names a viewer answers `panels`, and the members of
that viewer are inside `panels[0]`, so the line shows those members and the id of the
viewer that the client keeps for the next word. Every line
starts with `frame-source-agent: ` and holds none of the words that Horizon reads as a
working agent (crates/horizon-core/src/agents.rs), so the panel shows the state of a
scripted client and not a spinner.

Run it as the `command` of an agent-kind panel of the device smoke fixture, as
scripts/device-smoke/serve.py --frame-source-agent writes it. Horizon appends its own
launch arguments to the command of an agent panel, so the launcher passes `"$@"`
through and `parse_arguments` ignores every argument it does not know.

Standard library only, Python 3.10+, no network.
"""
from __future__ import annotations

import argparse
import collections
import json
import os
import shlex
import subprocess
import sys
import threading
import time

PROGRAM = 'frame-source-agent'
MCP_COMMAND_ENV = 'FRAME_SOURCE_AGENT_MCP_COMMAND'
MCP_READ_TIMEOUT_SECONDS = 30.0
# The members of an answer that the procedure reads: the kind of the source, the
# evidence of the transport and of the screen, and the counters of the program.
EVIDENCE_FIELDS = (
    'status', 'panel_id', 'kind', 'endpoint', 'visible', 'owned_by_caller', 'connection',
    'connection_error', 'image_received', 'image_displayed', 'frame_sequence',
    'received_frame_sequence', 'frames_dropped', 'age_ms', 'code', 'message',
)
DEFAULT_WATCH_COUNT = 3
WATCH_INTERVAL_SECONDS = 1.0


class Fatal(RuntimeError):
    """A failure that ends the script with a nonzero exit code."""


class StdioMcpClient:
    """One MCP server child, spoken to with newline-delimited JSON-RPC 2.0."""

    def __init__(self, command: str):
        argv = shlex.split(command)
        if not argv:
            raise Fatal('--mcp-command is empty')
        self._buffer = b''
        self._next_id = 0
        self._stderr = collections.deque(maxlen=40)
        try:
            # The identity that Horizon injected into this panel is inherited: the
            # server needs HORIZON_BROWSER_ACTOR and HORIZON_BROWSER_HOST_INSTANCE
            # to reach this workspace.
            self.proc = subprocess.Popen(
                argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                stderr=subprocess.PIPE, bufsize=0, close_fds=True)
        except OSError as error:
            raise Fatal(f'cannot start the MCP server {argv[0]!r}: {error}') from error
        threading.Thread(target=self._pump_stderr, daemon=True).start()
        self.initialize()

    def _pump_stderr(self) -> None:
        stream = self.proc.stderr
        if stream is None:
            return
        for raw in iter(stream.readline, b''):
            self._stderr.append(raw.decode('utf-8', 'replace').rstrip())

    def _write(self, value: dict) -> None:
        assert self.proc.stdin is not None
        self.proc.stdin.write(json.dumps(value).encode('utf-8') + b'\n')
        self.proc.stdin.flush()

    def _read(self) -> dict:
        stream = self.proc.stdout
        assert stream is not None
        deadline = time.monotonic() + MCP_READ_TIMEOUT_SECONDS
        while b'\n' not in self._buffer:
            if time.monotonic() > deadline:
                raise Fatal(f'the MCP server answered nothing in {MCP_READ_TIMEOUT_SECONDS:.0f} s')
            chunk = os.read(stream.fileno(), 4096)
            if not chunk:
                raise Fatal('the MCP server ended')
            self._buffer += chunk
        line, self._buffer = self._buffer.split(b'\n', 1)
        return json.loads(line.decode('utf-8'))

    def request(self, method: str, params: dict | None = None) -> dict:
        self._next_id += 1
        identifier = self._next_id
        message = {'jsonrpc': '2.0', 'id': identifier, 'method': method}
        if params is not None:
            message['params'] = params
        self._write(message)
        while True:
            answer = self._read()
            if answer.get('id') != identifier:
                continue
            if 'error' in answer:
                raise Fatal(f'{method} answered {answer["error"]}')
            return answer.get('result') or {}

    def notify(self, method: str) -> None:
        self._write({'jsonrpc': '2.0', 'method': method})

    def initialize(self) -> None:
        self.request('initialize', {
            'protocolVersion': '2024-11-05',
            'capabilities': {},
            'clientInfo': {'name': PROGRAM, 'version': '1'},
        })
        self.notify('notifications/initialized')

    def call_tool(self, name: str, arguments: dict) -> dict:
        result = self.request('tools/call', {'name': name, 'arguments': arguments})
        text = ' '.join(part.get('text', '') for part in result.get('content', [])
                        if isinstance(part, dict) and part.get('type') == 'text')
        structured = result.get('structuredContent')
        if isinstance(structured, dict):
            answer = dict(structured)
        elif text.strip():
            try:
                answer = json.loads(text)
            except json.JSONDecodeError as error:
                raise Fatal(f'{name} answered text that is not JSON: {error}') from error
        else:
            answer = {}
        if result.get('isError') and answer.get('status') != 'failed':
            answer = {'status': 'failed', 'message': text.strip() or 'the tool answered an error'}
        return answer

    def close(self) -> None:
        if self.proc.stdin is not None:
            try:
                self.proc.stdin.close()
            except OSError:
                pass
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.proc.terminate()


def viewer_of(answer: dict) -> dict:
    """The viewer that one answer names.

    An answer of `device_panel` is one `Outcome` of
    crates/horizon-browser-control/src/manifest/device.rs, and the operations that
    name a viewer answer `{"status": "panels", "panels": [<viewer>, ...]}`: the id
    of the viewer and its evidence are the members of `panels[0]`, never members
    of the top of the answer. An answer that holds no list, such as `close`, names
    the viewer itself.
    """
    panels = answer.get('panels')
    if isinstance(panels, list) and panels and isinstance(panels[0], dict):
        return panels[0]
    return answer


def evidence(answer: dict) -> dict:
    """The members of one answer that the procedure reads, and no other."""
    viewer = viewer_of(answer)
    # The status of an answer is a member of the answer itself, and the members of
    # the viewer that it names are the evidence of the viewer.
    kept = {'status': answer['status']} if 'status' in answer else {}
    kept.update({name: viewer[name] for name in EVIDENCE_FIELDS if name in viewer})
    server = viewer.get('server')
    if isinstance(server, dict) and 'desktop_size' in server:
        kept['desktop_size'] = server['desktop_size']
    if isinstance(answer.get('panels'), list):
        kept['panels'] = len(answer['panels'])
    return kept


class Session:
    """One agent panel: the last viewer that it created, and the tool of the host."""

    def __init__(self, client: StdioMcpClient):
        self.client = client
        self.panel_id: str | None = None

    def call(self, arguments: dict) -> dict:
        return self.client.call_tool('device_panel', arguments)

    def word(self, word: str, arguments: list[str]) -> dict:
        if word == 'create':
            if not arguments:
                return {'status': 'failed', 'message': 'create needs the name of a program'}
            request: dict = {'operation': 'create', 'frame_source': {'name': arguments[0]}}
            if len(arguments) > 1:
                request['frame_source']['target'] = arguments[1]
            answer = self.call(request)
            viewer = viewer_of(answer)
            if isinstance(viewer.get('panel_id'), str):
                self.panel_id = viewer['panel_id']
            return answer
        if word == 'list':
            return self.call({'operation': 'list'})
        if self.panel_id is None:
            return {'status': 'failed', 'message': f'{word} needs a viewer: type create first'}
        if word == 'inspect':
            return self.call({'operation': 'inspect', 'panel_id': self.panel_id})
        if word in ('hide', 'show'):
            return self.call({'operation': 'visibility', 'panel_id': self.panel_id,
                              'visible': word == 'show'})
        if word == 'close':
            answer = self.call({'operation': 'close', 'panel_id': self.panel_id})
            if answer.get('status') == 'closed':
                self.panel_id = None
            return answer
        return {'status': 'failed', 'message': f'{word} is not one word of this client'}

    def run_word(self, line: str) -> None:
        words = line.split()
        if not words:
            return
        word = words[0].lower()
        if word == 'watch':
            count = DEFAULT_WATCH_COUNT
            if len(words) > 1 and words[1].isdigit():
                count = max(1, int(words[1]))
            for index in range(count):
                print_answer(f'watch {index + 1}/{count}', self.word('inspect', []))
                if index + 1 < count:
                    time.sleep(WATCH_INTERVAL_SECONDS)
            return
        print_answer(word, self.word(word, words[1:]))


def print_answer(word: str, answer: dict) -> None:
    """One line of evidence, safe for the state scrape of the host."""
    print(f'{PROGRAM}: {word} -> {json.dumps(evidence(answer), sort_keys=True)}', flush=True)


def parse_arguments(argv=None):
    """The options of one run; arguments of the host that this script does not know
    are ignored."""
    parser = argparse.ArgumentParser(description='A scripted client of the device_panel tool')
    parser.add_argument(
        '--mcp-command', default=os.environ.get(MCP_COMMAND_ENV, ''),
        help='Command line of the Horizon MCP server, such as "<horizon> --browser-mcp". '
             f'Defaults to the {MCP_COMMAND_ENV} environment variable.')
    known, _unknown = parser.parse_known_args(argv)
    return known


def main(argv=None) -> int:
    """Read one word per line until the standard input of the panel ends."""
    options = parse_arguments(argv)
    client = StdioMcpClient(options.mcp_command)
    session = Session(client)
    print(f'{PROGRAM}: ready, type create <name> [target], inspect, watch [n], hide, show, '
          'close or list', flush=True)
    try:
        for line in sys.stdin:
            session.run_word(line)
    except (KeyboardInterrupt, BrokenPipeError):
        pass
    finally:
        client.close()
    return 0


if __name__ == '__main__':
    sys.exit(main())
