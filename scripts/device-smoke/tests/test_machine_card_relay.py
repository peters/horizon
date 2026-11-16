#!/usr/bin/env python3
"""Self test of the relay program that the machines skill ships.

The repository runs it with the other device smoke tests:

    python3 -B -m unittest discover -s scripts/device-smoke/tests -v

The relay is the one program that the setup writes on a machine, and it is the
reason a card link survives: an agent program starts it once, and it outlives a
lost link. This test reads that program out of the shipped `SKILL.md`, so the text
that a person writes on their machine is the text of this test, and proves:

  * it waits for a socket that appears late, and passes a call through it;
  * a server that closes answers the calls in flight with the error `-32000`;
  * after the reconnect the server sees `initialize` again, and the agent side sees
    no second answer of it;
  * a call that the agent sends while there is no link goes after the `initialize`
    of the reconnect, and its answer is the only one of that call;
  * a server that RESETS the stream is a lost link, and a call after it works;
  * a call after the reconnect is answered;
  * it ends with the standard input of the agent.

Standard library only. The relay writes into the folder of the test, through
`HORIZON_CARDS_DIR`, and reads a stand-in `tmux` of the test's `PATH`.
"""
from __future__ import annotations

import json
import os
import socket
import subprocess
import sys
import tempfile
import threading
import time
import unittest
import unittest.mock
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
SKILLS = (
    ROOT / 'assets/plugins/claude-code/skills/horizon-machines/SKILL.md',
    ROOT / 'assets/plugins/codex/skills/horizon-machines/SKILL.md',
)
# The two copies of the skill hold one text, so a person reads the same program on
# either agent program.
SESSION = 'hz-relay'
MACHINE = 'build-1'
DEADLINE = 20.0


def shipped_relay(path: Path) -> str:
    """The relay program of the skill: the python block that starts with its shebang."""
    text = path.read_text(encoding='utf-8')
    for block in text.split('```python\n')[1:]:
        body = block.split('\n```\n', 1)[0]
        if body.startswith('#!/usr/bin/env python3') and 'HORIZON_CARDS_DIR' in body:
            return body + '\n'
    raise AssertionError(f'no relay program in {path}')


def wait_for(what, wanted, deadline=DEADLINE):
    """Waits until `wanted` holds, and names what the test waited for."""
    end = time.monotonic() + deadline
    while time.monotonic() < end:
        held = wanted()
        if held:
            return held
        time.sleep(0.02)
    raise AssertionError(f'the test waited for {what}')


class CardServer(threading.Thread):
    """The far end of the card socket: it accepts, reads the calls and answers them.

    `silent` keeps a call without an answer, as a link that a closed laptop cuts
    does. `cut` closes the stream of that number, and the next accept is a new
    stream, which is what the `sshd` of the machine gives after a reconnect.
    `reset` ends the stream of that number with `shutdown(SHUT_RD)` and a close, as
    a forward that a machine drops does: the relay reads `ConnectionResetError`.
    """

    def __init__(self, path: Path):
        super().__init__(daemon=True)
        self.path = path
        self.calls = []
        self.streams = 0
        self.silent = False
        self.cut_stream = None
        self.pause = False
        self.reset_stream = None
        self.stopped = threading.Event()
        self.ready = threading.Event()

    def run(self):
        listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        listener.bind(str(self.path))
        listener.listen(4)
        listener.settimeout(0.1)
        self.ready.set()
        while not self.stopped.is_set():
            try:
                held, _ = listener.accept()
            except socket.timeout:
                continue
            except OSError:
                break
            self.streams += 1
            self.serve(held, self.streams)
        listener.close()

    def serve(self, held, stream):
        """One stream of the socket: the calls of the relay, and its answers."""
        held.settimeout(0.05)
        buffer = b''
        while not self.stopped.is_set() and self.cut_stream != stream:
            if self.reset_stream == stream:
                # One call of the relay stays unread in the queue of this server, and
                # the close then resets the stream instead of ending it, as a forward
                # that a machine drops does.
                while not self.peek(held):
                    pass
                break
            if self.pause:
                time.sleep(0.05)
                continue
            try:
                data = held.recv(65536)
            except socket.timeout:
                continue
            except OSError:
                break
            if not data:
                break
            buffer += data
            while b'\n' in buffer:
                line, buffer = buffer.split(b'\n', 1)
                if not line.strip():
                    continue
                message = json.loads(line)
                self.calls.append((stream, message))
                if self.silent or 'id' not in message:
                    continue
                answer = {'jsonrpc': '2.0', 'id': message['id'], 'result': {'echo': message.get('method')}}
                held.sendall(json.dumps(answer).encode() + b'\n')
        try:
            if self.reset_stream == stream:
                held.shutdown(socket.SHUT_RD)
            held.close()
        except OSError:
            pass

    @staticmethod
    def peek(held) -> bool:
        """Whether a call of the relay waits unread in the queue of this stream."""
        try:
            return bool(held.recv(65536, socket.MSG_PEEK))
        except (socket.timeout, OSError):
            return False

    def seen(self, method, stream=None):
        """The calls of one method, in the streams that the test names."""
        return [call for number, call in self.calls
                if call.get('method') == method and (stream is None or number == stream)]

    def stop(self):
        self.stopped.set()
        self.join(DEADLINE)


def relay_child(folder: Path, script: Path) -> subprocess.Popen:
    """One relay of the test: its stand-in `tmux`, its folder and its pipes."""
    bin_folder = folder / 'bin'
    bin_folder.mkdir(mode=0o700, exist_ok=True)
    tmux = bin_folder / 'tmux'
    tmux.write_text(f'#!/bin/sh\nprintf "%s\\n" {SESSION}\n', encoding='utf-8')
    tmux.chmod(0o700)
    return subprocess.Popen(
        [sys.executable, '-u', str(script), MACHINE],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        env=dict(os.environ, HORIZON_CARDS_DIR=str(folder / 'horizon-cards'),
                 PATH=str(bin_folder) + os.pathsep + os.environ.get('PATH', '')))


class AgentSide:
    """The standard input and output of one relay: what its agent program says."""

    def __init__(self, child: subprocess.Popen):
        self.child = child
        self.answers = []
        threading.Thread(target=self.read, daemon=True).start()

    def read(self):
        """One reader for the whole test: two of them would take each other's lines."""
        while True:
            line = self.child.stdout.readline()
            if not line:
                return
            self.answers.append(json.loads(line))

    def say(self, message: dict) -> None:
        """One line to the relay, as the agent writes it."""
        self.child.stdin.write(json.dumps(message).encode() + b'\n')
        self.child.stdin.flush()

    def hear(self, wanted) -> dict:
        """Waits for one answer of the relay that `wanted` names."""
        return wait_for('an answer of the relay', lambda: next((one for one in self.answers if wanted(one)), None))


class RelayTest(unittest.TestCase):
    """One relay of the shipped text, one socket of the test."""

    def setUp(self):
        self.folder = Path(tempfile.mkdtemp(prefix='hz-relay-'))
        (self.folder / 'horizon-cards' / MACHINE).mkdir(parents=True, mode=0o700)
        self.script = self.folder / 'relay.py'
        self.script.write_text(shipped_relay(SKILLS[0]), encoding='utf-8')
        self.script.chmod(0o700)
        self.socket = self.folder / 'horizon-cards' / MACHINE / f'{SESSION}.sock'
        self.server = None
        self.agent = None
        self.child = None
        # A test that closes the standard input of the relay waits for its end, so
        # an end before that is the failure of the relay.
        self.closed_stdin = False

    def tearDown(self):
        if self.child is not None:
            if self.child.poll() is not None and not self.closed_stdin:
                self.fail(f'the relay ended by itself: {self.child.stderr.read().decode()}')
            self.child.stdin.close()
            try:
                self.child.wait(timeout=DEADLINE)
            except subprocess.TimeoutExpired:
                self.child.kill()
                self.child.wait(timeout=DEADLINE)
            for pipe in (self.child.stdout, self.child.stderr):
                if pipe is not None:
                    pipe.close()
        if self.server is not None:
            self.server.stop()
        if self.socket.exists():
            self.socket.unlink()

    def start_relay(self):
        self.child = relay_child(self.folder, self.script)
        self.agent = AgentSide(self.child)
        return self.agent

    def start_server(self):
        self.server = CardServer(self.socket)
        self.server.start()
        wait_for('the socket of the test', self.socket.exists)

    def test_the_two_copies_of_the_skill_ship_one_program(self):
        self.assertEqual(shipped_relay(SKILLS[0]), shipped_relay(SKILLS[1]), 'one relay for both skills')
        text = shipped_relay(SKILLS[0])
        self.assertLessEqual(len(text.splitlines()), 80, 'the relay stays short')
        self.assertIn('"#S"', SKILLS[0].read_text(encoding='utf-8'), 'the nc line quotes the session in double quotes')

    def test_a_socket_that_appears_late_is_met_and_answers_a_call(self):
        agent = self.start_relay()
        agent.say({'jsonrpc': '2.0', 'id': 1, 'method': 'initialize',
                   'params': {'protocolVersion': '2025-06-18', 'capabilities': {},
                              'clientInfo': {'name': 'relay-test', 'version': '1'}}})
        # The socket appears after the relay tried it at least once.
        time.sleep(1.0)
        self.start_server()
        call = wait_for('the initialize of the agent on the socket', lambda: self.server.seen('initialize'))
        self.assertEqual(call[0]['id'], 1)
        answer = agent.hear(lambda message: message.get('id') == 1)
        self.assertEqual(answer['result'], {'echo': 'initialize'})

    def test_a_cut_link_answers_the_calls_in_flight_and_a_call_after_it_works(self):
        self.start_server()
        agent = self.start_relay()
        agent.say({'jsonrpc': '2.0', 'id': 1, 'method': 'initialize'})
        agent.hear(lambda message: message.get('id') == 1)
        agent.say({'jsonrpc': '2.0', 'method': 'notifications/initialized'})
        wait_for('the initialized line of the agent', lambda: self.server.seen('notifications/initialized'))

        # Two calls in flight, and the link ends before an answer.
        self.server.silent = True
        agent.say({'jsonrpc': '2.0', 'id': 2, 'method': 'tools/call'})
        agent.say({'jsonrpc': '2.0', 'id': 3, 'method': 'tools/call'})
        wait_for('the calls in flight', lambda: len(self.server.seen('tools/call')) == 2)
        self.server.silent = False
        self.server.cut_stream = 1

        errors = [agent.hear(lambda message, wanted=number: message.get('id') == wanted)
                  for number in (2, 3)]
        for error in errors:
            self.assertEqual(error['error']['code'], -32000)
            self.assertEqual(error['error']['message'], 'The link to Horizon was lost. Call again.')

        # The reconnect: one stream more, with the remembered initialize, and its
        # answer never reaches the agent.
        again = wait_for('the initialize of the reconnect', lambda: self.server.seen('initialize', 2))
        self.assertEqual(again[0]['id'], 1)
        wait_for('the initialized line of the reconnect', lambda: self.server.seen('notifications/initialized', 2))

        agent.say({'jsonrpc': '2.0', 'id': 4, 'method': 'tools/call'})
        answer = agent.hear(lambda message: message.get('id') == 4)
        self.assertEqual(answer['result'], {'echo': 'tools/call'})
        self.assertEqual(len(self.server.seen('initialize')), 2, 'the relay says hello again on the new stream')
        self.assertEqual(len([one for one in self.agent.answers if one.get('id') == 1]), 1,
                         'the agent saw one answer of its initialize')

    def test_a_call_that_waits_for_a_lost_link_follows_the_hello_of_the_reconnect(self):
        self.start_server()
        agent = self.start_relay()
        agent.say({'jsonrpc': '2.0', 'id': 1, 'method': 'initialize'})
        agent.hear(lambda message: message.get('id') == 1)
        agent.say({'jsonrpc': '2.0', 'method': 'notifications/initialized'})
        wait_for('the initialized line of the agent', lambda: self.server.seen('notifications/initialized'))

        # The link goes before the call: no socket to meet, as a forward of a machine
        # that ended leaves it.
        self.server.stop()
        self.server = None
        self.socket.unlink()
        agent.say({'jsonrpc': '2.0', 'id': 2, 'method': 'tools/call'})
        time.sleep(1.0)
        self.start_server()

        # The new stream says hello first, and only then the call of the cut link.
        wait_for('the call of the lost link on the new stream', lambda: self.server.seen('tools/call'))
        self.assertEqual(
            [message.get('method') for number, message in self.server.calls if number == 1],
            ['initialize', 'notifications/initialized', 'tools/call'],
            'the hello of the relay comes before the call that waited for the link')
        answer = agent.hear(lambda message: message.get('id') == 2)
        self.assertEqual(answer['result'], {'echo': 'tools/call'})
        self.assertEqual(len([one for one in agent.answers if one.get('id') == 1]), 1,
                         'the agent saw one answer of its initialize, and no second one')

    def test_a_stream_that_resets_is_a_lost_link(self):
        self.start_server()
        agent = self.start_relay()
        agent.say({'jsonrpc': '2.0', 'id': 1, 'method': 'initialize'})
        agent.hear(lambda message: message.get('id') == 1)
        agent.say({'jsonrpc': '2.0', 'method': 'notifications/initialized'})
        wait_for('the initialized line of the agent', lambda: self.server.seen('notifications/initialized'))

        # A call in flight, and one more that the server never reads: the close of the
        # server then resets the stream, and `recv` of the relay raises an `OSError`.
        self.server.silent = True
        agent.say({'jsonrpc': '2.0', 'id': 2, 'method': 'tools/call'})
        wait_for('the call in flight', lambda: self.server.seen('tools/call'))
        self.server.pause = True
        time.sleep(0.2)
        agent.say({'jsonrpc': '2.0', 'id': 3, 'method': 'tools/call'})
        self.server.reset_stream = 1
        error = agent.hear(lambda message: message.get('id') == 2)
        self.assertEqual(error['error']['code'], -32000)
        self.assertEqual(error['error']['message'], 'The link to Horizon was lost. Call again.')

        # The relay stays alive, meets the next stream of the same socket, and a call
        # there works.
        self.server.silent, self.server.pause = False, False
        wait_for('the hello of the reconnect', lambda: self.server.seen('initialize', 2))
        agent.say({'jsonrpc': '2.0', 'id': 4, 'method': 'tools/call'})
        answer = agent.hear(lambda message: message.get('id') == 4)
        self.assertEqual(answer['result'], {'echo': 'tools/call'})

    def test_a_call_that_waits_on_a_live_link_is_not_cut(self):
        """`next_click` waits for a person for minutes: the bound of a call is for a lost link only."""
        self.start_server()
        with unittest.mock.patch.dict(os.environ, {'HORIZON_CARDS_WAIT': '1'}):
            agent = self.start_relay()
        agent.say({'jsonrpc': '2.0', 'id': 1, 'method': 'initialize'})
        agent.hear(lambda message: message.get('id') == 1)
        self.server.silent = True
        agent.say({'jsonrpc': '2.0', 'id': 2, 'method': 'tools/call'})
        wait_for('the call that waits', lambda: self.server.seen('tools/call'))
        # Three times the bound of a lost link, with the link up the whole time.
        time.sleep(3.0)
        self.assertEqual([one for one in agent.answers if one.get('id') == 2], [],
                         'a call that waits on a live link gets no error')
        self.assertEqual(self.server.streams, 1, 'the relay kept its one stream')

    def test_the_relay_ends_with_the_stdin_of_the_agent(self):
        self.start_server()
        agent = self.start_relay()
        agent.say({'jsonrpc': '2.0', 'id': 1, 'method': 'initialize'})
        agent.hear(lambda message: message.get('id') == 1)
        self.closed_stdin = True
        self.child.stdin.close()
        self.assertEqual(self.child.wait(timeout=DEADLINE), 0, 'the relay ends when its stdin ends')


if __name__ == '__main__':
    unittest.main()
