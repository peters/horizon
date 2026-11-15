#!/usr/bin/env python3
"""The agent of a machine: one card in Horizon, drawn through the card socket.

`serve.py --machine-watch` runs this program in one session of its private `tmux`
server, where the person sees it, and it speaks MCP to the socket that the stand-in
`ssh` forwards from that session's path on the "machine". It draws one card and
keeps its `next_click` call open, which is the loop that a real agent of a machine
runs there.

The card holds a heading, one line and two buttons. A press comes back as the
component that the person pressed, and the agent then updates the card with the
label of that press. Nothing of Horizon is on this side: one socket, one protocol,
and the identity arrives from the socket that Horizon chose.

Standard library only. Nothing is written outside the socket.
"""
from __future__ import annotations

import argparse
import json
import socket
import subprocess
import sys
import time

# The card of the run: a heading, one line and the two buttons that a person sees. The
# title names the session, so a person reads which stream drew the card.
DEFAULT_TITLE = 'Night shift'


def card(title: str, line: str | None = None) -> dict:
    """One card of this agent: a heading of `title`, one line and two buttons."""
    return {
        'components': [
            {'type': 'heading', 'text': title, 'subtitle': 'an agent on the machine'},
            {'type': 'text', 'text': line or 'This card came through the socket of the session, not from a panel of Horizon.'},
            {'type': 'buttons', 'items': [
                {'id': 'continue', 'label': 'Continue', 'tone': 'primary'},
                {'id': 'stop', 'label': 'Stop', 'tone': 'danger'}]},
        ]
    }

# How long the agent waits for the socket of its session, in seconds. Horizon starts
# the forward of a machine after its watch reports the session, so the first try can
# come before the socket exists.
CONNECT_TIMEOUT_SECONDS = 60.0
CONNECT_POLL_SECONDS = 0.2

# The bound of one call that the person never answers: `next_click` of the agent ends
# it, and the agent asks again.
CLICK_TIMEOUT_MILLIS = 120_000


def log(line: str) -> None:
    """One line in the pane of the agent, which the person reads there."""
    print(f'machine-card-agent: {line}', flush=True)


class SocketClient:
    """One MCP server on a Unix socket, spoken to with newline-delimited JSON-RPC."""

    def __init__(self, path: str | None, deadline: float, relay: tuple[str, str] | None = None):
        self.path = path
        self._buffer = b''
        self._next_id = 0
        self.sock = RelayChannel(*relay) if relay else self._connect(deadline)

    def _connect(self, deadline: float) -> socket.socket:
        while True:
            held = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            try:
                held.connect(self.path)
                return held
            except OSError as error:
                held.close()
                if time.monotonic() >= deadline:
                    raise
                log(f'waiting for {self.path} ({error})')
                time.sleep(CONNECT_POLL_SECONDS)

    def close(self) -> None:
        self.sock.close()

    def _read_line(self) -> dict:
        while b'\n' not in self._buffer:
            chunk = self.sock.recv(65536)
            if not chunk:
                raise ConnectionError('the socket of the card closed')
            self._buffer += chunk
        line, self._buffer = self._buffer.split(b'\n', 1)
        return json.loads(line) if line.strip() else self._read_line()

    def _request(self, method: str, params: dict | None = None) -> dict:
        self._next_id += 1
        message = {'jsonrpc': '2.0', 'id': self._next_id, 'method': method}
        if params is not None:
            message['params'] = params
        self.sock.sendall(json.dumps(message, separators=(',', ':')).encode('utf-8') + b'\n')
        while True:
            answer = self._read_line()
            if answer.get('id') == self._next_id:
                return answer

    def initialize(self) -> None:
        self._request('initialize', {
            'protocolVersion': '2025-06-18',
            'capabilities': {},
            'clientInfo': {'name': 'machine-card-agent', 'version': '1'},
        })
        self.sock.sendall(
            json.dumps({'jsonrpc': '2.0', 'method': 'notifications/initialized'},
                       separators=(',', ':')).encode('utf-8') + b'\n')

    def call_tool(self, arguments: dict) -> dict:
        answer = self._request('tools/call', {'name': 'block_panel', 'arguments': arguments})
        if 'error' in answer:
            raise RuntimeError(answer['error'].get('message', 'the call failed'))
        payload = answer['result'].get('structuredContent')
        return payload if isinstance(payload, dict) else answer['result']


class RelayChannel:
    """The relay program of the machines skill, as one channel of the same calls.

    The relay waits for the socket of its session itself, so this makes no connect
    loop and no restart: it is the program that the setup of a machine writes, and
    the reason an agent program starts one entry and keeps it.
    """

    def __init__(self, script: str, machine: str):
        self.child = subprocess.Popen([sys.executable, '-u', script, machine],
                                      stdin=subprocess.PIPE, stdout=subprocess.PIPE)

    def recv(self, size: int) -> bytes:
        return self.child.stdout.read1(size)

    def sendall(self, data: bytes) -> None:
        self.child.stdin.write(data)
        self.child.stdin.flush()

    def close(self) -> None:
        self.child.stdin.close()
        try:
            self.child.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.child.kill()


def draw(client: SocketClient, title: str) -> str:
    """Creates the card and returns its panel id."""
    payload = client.call_tool({'operation': 'create', 'title': title, 'document': card(title)})
    if payload.get('status') != 'created':
        raise RuntimeError(f'the host refused the card: {payload}')
    log(f'card {payload["panel_id"]} is drawn')
    return payload['panel_id']


def wait_for_click(client: SocketClient, panel_id: str, title: str) -> None:
    """Keeps `next_click` open and updates the card with the label of each press."""
    while True:
        payload = client.call_tool({
            'operation': 'next_click',
            'panel_id': panel_id,
            'timeout_millis': CLICK_TIMEOUT_MILLIS,
        })
        if payload.get('status') != 'clicked':
            log(f'the wait ended: {payload.get("status")}')
            continue
        # The press is the object `click` of the answer, as the block skill states it.
        pressed = str((payload.get('click') or {}).get('component_id', ''))
        label = {'continue': 'Continue', 'stop': 'Stop'}.get(pressed, pressed)
        log(f'the person pressed {label}')
        updated = client.call_tool({
            'operation': 'update',
            'panel_id': panel_id,
            'document': card(title, f'The person pressed {label}.'),
        })
        if updated.get('status') != 'updated':
            log(f'the update was refused: {updated}')


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description='Draw and update one card of a machine session.')
    parser.add_argument('--socket', help='the card socket of this tmux session')
    parser.add_argument('--relay', nargs=2, metavar=('SCRIPT', 'MACHINE'),
                        help='the relay program of the machines skill, and the name of the machine')
    parser.add_argument('--title', default=DEFAULT_TITLE, help='the heading of the card')
    parser.add_argument('--once', action='store_true',
                        help='draw the card and end, which a test of this program asks for')
    parser.add_argument('--connect-timeout', type=float, default=CONNECT_TIMEOUT_SECONDS,
                        help='how long the agent waits for the socket of its session, in seconds')
    arguments = parser.parse_args(argv)
    if (arguments.socket is None) == (arguments.relay is None):
        parser.error('name one of --socket and --relay')
    client = SocketClient(arguments.socket, time.monotonic() + arguments.connect_timeout,
                          tuple(arguments.relay) if arguments.relay else None)
    try:
        client.initialize()
        panel_id = draw(client, arguments.title)
        if arguments.once:
            return 0
        wait_for_click(client, panel_id, arguments.title)
    except (ConnectionError, OSError, RuntimeError) as error:
        # A lost link ends the call: the agent says so and the session keeps the
        # line for the person who reads it.
        log(f'the card socket ended: {error}')
        return 1
    finally:
        client.close()
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
