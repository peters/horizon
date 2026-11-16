#!/usr/bin/env python3
"""Self test for machine_card_agent.py, and for the stand-in forward of that card.

The repository runs it with the other device smoke tests:

    python3 -B -m unittest discover -s scripts/device-smoke/tests -v

`serve.py --machine-watch` names a stand-in `ssh`, and the child that carries the
cards of a machine forwards one Unix socket of each session. This test proves:

  * the stand-in `ssh` reads `-R <socket of the machine>:<socket here>` out of its
    arguments, binds the first path and copies every connection of it to the second
    one, in both directions;
  * `machine_card_agent.py` speaks MCP over such a socket: it sends `initialize`,
    then `block_panel create` with a card of two buttons, and `--once` ends it;
  * a click that the server answers in the shape of the block skill leads to one
    `block_panel update` whose document names the label that the person pressed;
  * a server that cuts the stream ends no call: the agent meets the socket again,
    says `initialize` again, asks the same card for its next press and draws no
    second card, and that press becomes an update;
  * the relay answers one call with the error of a lost link, and the agent calls
    again and goes on with the card that it drew;
  * a socket that never answers ends the program with one line and a status;
  * the machine record that the fixture writes carries the mark of the cards, which
    is the field that the record reader in core reads.

Standard library only. Nothing is written outside the folder of a test.
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
from pathlib import Path

HERE = Path(__file__).resolve().parent
FIXTURE = HERE.parent
ROOT = FIXTURE.parents[1]
AGENT = FIXTURE / 'machine_card_agent.py'
sys.path.insert(0, str(FIXTURE))
import serve  # noqa: E402

# How long a test waits for a socket or for a line of the agent, in seconds.
DEADLINE = 20.0

# The answer of one press, as the block skill prints it
# (`assets/plugins/claude-code/skills/horizon-block/SKILL.md`, "Read a click"), with
# the panel and the revision of the card of this test. The press is the object
# `click`, and the button that the person hit is its `component_id`.
CLICK_ANSWER = {'status': 'clicked', 'click': {
    'event_id': 4, 'panel_id': 'card-1', 'component_id': 'continue',
    'revision': 1, 'selected': [], 'at_millis': 1700000000000, 'stale': False}}

# The error that the relay program of the machines skill answers a call with when the
# link to Horizon ended, as `assets/plugins/claude-code/skills/horizon-machines/SKILL.md`
# states it.
LOST = 'The link to Horizon was lost. Call again.'

# The file of this test serves as the relay of the relay lane, which is the idiom of
# the fake servers of this folder: a run with one argument is that relay.
THIS_FILE = Path(__file__).resolve()


def wait_for(what, wanted, deadline=DEADLINE):
    """Waits until `wanted` holds, and names what the test waited for."""
    end = time.monotonic() + deadline
    while time.monotonic() < end:
        held = wanted()
        if held:
            return held
        time.sleep(0.02)
    raise AssertionError(f'the test waited for {what}')


class Bridge:
    """The stand-in forward of one run: a program from `serve.py` and its child."""

    def __init__(self, folder: Path, pairs: list[tuple[str, str]]):
        self.program = folder / serve.MACHINE_CARD_BRIDGE
        self.program.write_text(serve.MACHINE_CARD_BRIDGE_BODY)
        self.program.chmod(0o700)
        self.log = folder / f'{serve.MACHINE_CARD_BRIDGE}.log'
        with self.log.open('a') as words:
            self.child = subprocess.Popen([sys.executable, '-u', str(self.program), *(
                f'{remote}:{local}' for remote, local in pairs)], stdout=words,
                stderr=subprocess.STDOUT)
        self.pairs = pairs

    def says(self) -> str:
        """The words of the bridge of this run, as a person reads them in its pane."""
        return self.log.read_text(encoding='utf-8') if self.log.exists() else ''

    def stop(self) -> None:
        self.child.terminate()
        self.child.wait(timeout=10)

    def binds(self) -> bool:
        return all(Path(remote).exists() for remote, _ in self.pairs)


class OneServer(threading.Thread):
    """One MCP server on a Unix socket, which answers from `answers`.

    `cut_after` answers that many `tools/call` calls and then ends the stream without
    an answer for the next one, which is the cut of a link. The server then accepts
    the next stream of the agent, as the `sshd` of a machine does after a reconnect,
    and `streams` counts the connections of the agent.
    """

    def __init__(self, path: Path, answers: list[dict], cut_after: int | None = None):
        super().__init__(daemon=True)
        self.path = path
        self.answers = answers
        self.cut_after = cut_after
        self.cut_stream: int | None = None
        self.calls: list[dict] = []
        self.answered = 0
        self.streams = 0
        self.ready = threading.Event()
        self.done = threading.Event()
        self.finished = threading.Event()

    def run(self) -> None:
        path = Path(self.path)
        try:
            path.unlink()
        except FileNotFoundError:
            pass
        path.parent.mkdir(parents=True, exist_ok=True)
        listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        listener.bind(str(path))
        listener.listen(4)
        self.ready.set()
        try:
            while not self.finished.is_set():
                held, _ = listener.accept()
                self.streams += 1
                self.serve(held)
        finally:
            listener.close()
            self.done.set()

    def serve(self, held: socket.socket) -> None:
        """One stream of the socket: the calls of the agent, and its answers."""
        stream = held.makefile('rwb')
        try:
            for line in stream:
                message = json.loads(line)
                if 'id' not in message:
                    continue
                self.calls.append(message)
                # The server of a test holds one answer for each call that it answers;
                # the socket closes when the answers end, which ends the agent. A cut
                # stream answers nothing, so the call after it takes the next answer.
                if (self.cut_after is not None and self.cut_stream is None
                        and len(self.tool_calls()) > self.cut_after):
                    self.cut_stream = self.streams
                    return
                answer = {'jsonrpc': '2.0', 'id': message['id'], 'result': self.answer(message)}
                stream.write(json.dumps(answer).encode('utf-8') + b'\n')
                stream.flush()
                if message['method'] != 'tools/call':
                    continue
                self.answered += 1
                if self.answered >= len(self.answers):
                    self.finished.set()
                    return
        finally:
            stream.close()
            held.close()

    def answer(self, message: dict) -> dict:
        if message['method'] == 'initialize':
            return {'protocolVersion': '2025-06-18', 'capabilities': {}, 'serverInfo': {'name': 'test', 'version': '1'}}
        return {'structuredContent': self.answers[self.answered]}

    def tool_calls(self) -> list[dict]:
        """The `block_panel` operations that the agent asked for, in order."""
        return [message['params']['arguments'] for message in self.calls
                if message['method'] == 'tools/call']


class Pane(threading.Thread):
    """The lines of the pane of one agent, which the test reads while it runs."""

    def __init__(self, child: subprocess.Popen):
        super().__init__(daemon=True)
        self.child = child
        self.lines: list[str] = []
        self.start()

    def run(self) -> None:
        for line in self.child.stdout:
            self.lines.append(line)

    def says(self, words: str) -> bool:
        """Whether one line of the pane holds `words`."""
        return any(words in line for line in self.lines)


class MachineCardAgentTest(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory(prefix='hz-machine-card-')
        self.addCleanup(self.folder.cleanup)
        self.root = Path(self.folder.name)

    def start_agent(self, *words: str, env: dict | None = None) -> subprocess.Popen:
        child = subprocess.Popen(
            [sys.executable, '-u', str(AGENT), *words],
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, env=env)
        self.addCleanup(lambda: child.poll() is None and child.kill())
        return child

    def stop_agent(self, child: subprocess.Popen) -> str:
        """Ends one agent of the test, and returns the words of its pane."""
        child.terminate()
        output, _ = child.communicate(timeout=30)
        return output

    def test_the_forward_of_a_machine_copies_a_stream_both_ways(self):
        here = self.root / 'here'
        there = self.root / 'there'
        here.parent.mkdir(parents=True, exist_ok=True)
        listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        listener.bind(str(here))
        listener.listen(1)
        self.addCleanup(listener.close)
        pair = (str(there / 'build-1' / 'hz-night.sock'), str(here))
        bridge = Bridge(self.root, [pair])
        self.addCleanup(bridge.stop)
        wait_for('the socket of the machine', bridge.binds)

        near = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        near.connect(pair[0])
        self.addCleanup(near.close)
        far, _ = listener.accept()
        self.addCleanup(far.close)

        near.sendall(b'hello\n')
        self.assertEqual(wait_for('the bytes here', lambda: far.recv(64)), b'hello\n')
        far.sendall(b'world\n')
        self.assertEqual(wait_for('the bytes there', lambda: near.recv(64)), b'world\n')

    def test_the_agent_draws_one_card_and_ends_after_it(self):
        socket_path = self.root / 'cards' / 'build-1' / 'hz-night.sock'
        server = OneServer(socket_path, [{'status': 'created', 'panel_id': 'card-1', 'revision': 1}])
        server.start()
        wait_for('the socket of the server', lambda: server.ready.is_set())
        agent = self.start_agent('--socket', str(socket_path), '--once')
        output, _ = agent.communicate(timeout=30)
        self.assertEqual(agent.returncode, 0, output)
        self.assertIn('card-1', output)

        calls = server.tool_calls()
        self.assertEqual([call['operation'] for call in calls], ['create'], calls)
        document = calls[0]['document']
        kinds = [component['type'] for component in document['components']]
        self.assertEqual(kinds, ['heading', 'text', 'buttons'], kinds)
        labels = [item['label'] for item in document['components'][-1]['items']]
        self.assertEqual(labels, ['Continue', 'Stop'], labels)

    def test_a_click_becomes_an_update_with_the_label_of_the_press(self):
        socket_path = self.root / 'cards' / 'build-1' / 'hz-night.sock'
        server = OneServer(socket_path, [
            {'status': 'created', 'panel_id': 'card-1', 'revision': 1},
            CLICK_ANSWER,
            {'status': 'updated', 'panel_id': 'card-1', 'revision': 2},
        ])
        server.start()
        wait_for('the socket of the server', lambda: server.ready.is_set())
        child = self.start_agent('--socket', str(socket_path))

        wait_for('the update of the click', lambda: len(server.tool_calls()) >= 3)
        calls = server.tool_calls()
        self.assertEqual([call['operation'] for call in calls[:3]], ['create', 'next_click', 'update'], calls)
        self.assertEqual(calls[1]['panel_id'], 'card-1')
        texts = [component.get('text', '') for component in calls[2]['document']['components']]
        self.assertIn('The person pressed Continue.', texts)
        wait_for('the end of the agent', lambda: server.done.is_set() or child.poll() is not None)

    def test_a_socket_that_never_answers_says_so_and_ends(self):
        missing = self.root / 'cards' / 'build-1' / 'nothing.sock'
        agent = self.start_agent('--socket', str(missing), '--once', '--connect-timeout', '1')
        output, _ = agent.communicate(timeout=60)
        self.assertEqual(agent.returncode, 1, output)
        self.assertIn('machine-card-agent:', output)

    def test_a_cut_stream_ends_no_call_and_the_press_after_it_is_an_update(self):
        socket_path = self.root / 'cards' / 'build-1' / 'hz-night.sock'
        server = OneServer(socket_path, [
            {'status': 'created', 'panel_id': 'card-1', 'revision': 1},
            CLICK_ANSWER,
            {'status': 'updated', 'panel_id': 'card-1', 'revision': 2},
        ], cut_after=1)
        server.start()
        wait_for('the socket of the server', lambda: server.ready.is_set())
        child = self.start_agent('--socket', str(socket_path))

        # The first stream ends after the card, without an answer for the
        # `next_click` that follows: the agent meets the socket again and asks the
        # card that it drew for its next press.
        wait_for('the update after the cut', lambda: len(server.tool_calls()) >= 4)
        self.assertIsNone(child.poll(), 'the agent lives on after the cut')
        self.assertEqual(server.streams, 2, 'the agent connected again after the cut')
        calls = server.tool_calls()
        self.assertEqual([call['operation'] for call in calls[:4]],
                         ['create', 'next_click', 'next_click', 'update'], calls)
        self.assertEqual([call['panel_id'] for call in calls[1:4]],
                         ['card-1'] * 3, 'a cut draws no second card')
        texts = [component.get('text', '') for component in calls[3]['document']['components']]
        self.assertIn('The person pressed Continue.', texts)
        self.assertIn('the socket of the session ended', self.stop_agent(child))

    def test_an_error_answer_of_the_relay_ends_no_call(self):
        log = self.root / 'relay-calls.jsonl'
        env = dict(os.environ, FAKE_RELAY_LOG=str(log))
        child = self.start_agent('--relay', str(THIS_FILE), 'build-1', env=env)
        pane = Pane(child)

        wait_for('the call after the error', lambda: pane.says('the person pressed Continue'))
        self.assertIsNone(child.poll(), 'the agent lives on after the error answer')
        calls = [json.loads(line) for line in log.read_text(encoding='utf-8').splitlines()]
        arguments = [message['params']['arguments'] for message in calls
                     if message['method'] == 'tools/call']
        self.assertEqual([call['operation'] for call in arguments[:4]],
                         ['create', 'next_click', 'next_click', 'update'], arguments)
        texts = [component.get('text', '') for component in arguments[3]['document']['components']]
        self.assertIn('The person pressed Continue.', texts)
        self.assertTrue(pane.says('the call was answered with an error'), pane.lines)
        self.stop_agent(child)


class RelayOfTheSkillTest(unittest.TestCase):
    """The relay program of the machines skill, and the fault that a restart proves."""

    def setUp(self):
        self.folder = tempfile.TemporaryDirectory(prefix='hz-relay-')
        self.addCleanup(self.folder.cleanup)
        self.root = Path(self.folder.name)

    def test_the_fixture_runs_the_relay_program_of_the_skill(self):
        with tempfile.TemporaryDirectory(prefix='hz-relay-state-') as state:
            program = serve.machine_relay_program(state)
            shipped = serve.machine_relay_text()
            self.assertEqual(program.read_text(encoding='utf-8'), shipped,
                             'the fixture runs the relay that the skill ships')
            self.assertEqual(program.stat().st_mode & 0o777, 0o700, 'the relay of a person is 700')
            self.assertLessEqual(len(shipped.splitlines()), 80, 'the relay stays short')
            self.assertIn(serve.MACHINE_RELAY_SESSION, 'hz-relay')

    def test_the_agent_draws_its_card_through_the_shipped_relay(self):
        session = 'hz-relay'
        socket_path = self.root / 'horizon-cards' / 'build-1' / f'{session}.sock'
        server = OneServer(socket_path, [{'status': 'created', 'panel_id': 'card-relay', 'revision': 1}])
        server.start()
        wait_for('the socket of the server', lambda: server.ready.is_set())
        relay = self.root / 'relay.py'
        relay.write_text(serve.machine_relay_text(), encoding='utf-8')
        relay.chmod(0o700)
        # The relay learns its session from the tmux client of the pane, which this
        # stand-in answers for a machine without a tmux server.
        programs = self.root / 'bin'
        programs.mkdir()
        tmux = programs / 'tmux'
        tmux.write_text(f'#!/bin/sh\nprintf "%s\\n" {session}\n', encoding='utf-8')
        tmux.chmod(0o700)
        child = subprocess.Popen(
            [sys.executable, '-u', str(AGENT), '--relay', str(relay), 'build-1',
             '--title', serve.MACHINE_RELAY_TITLE, '--once'],
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
            env=dict(os.environ, HORIZON_CARDS_DIR=str(self.root / 'horizon-cards'),
                     PATH=str(programs) + os.pathsep + os.environ.get('PATH', '')))
        output, _ = child.communicate(timeout=60)
        self.assertEqual(child.returncode, 0, output)
        self.assertIn('card-relay', output)
        calls = server.tool_calls()
        self.assertEqual([call['operation'] for call in calls], ['create'], calls)
        self.assertEqual(calls[0]['title'], serve.MACHINE_RELAY_TITLE)
        wait_for('the end of the server', lambda: server.done.is_set())

    def test_the_bridge_leaves_its_socket_file_and_refuses_it_again(self):
        remote = str(self.root / 'there' / 'build-1' / 'hz-night.sock')
        here = self.root / 'here'
        listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        listener.bind(str(here))
        listener.listen(2)
        self.addCleanup(listener.close)
        bridge = Bridge(self.root, [(remote, str(here))])
        wait_for('the socket of the machine', bridge.binds)
        bridge.stop()
        self.assertTrue(Path(remote).exists(), 'the file of a child that ended stays, as sshd leaves it')

        again = Bridge(self.root, [(remote, str(here))])
        self.addCleanup(again.stop)
        wait_for('the refusal of the taken path', lambda: 'is taken' in again.says())
        self.assertIn('hz-night.sock', again.says())

        # The removal that Horizon makes before its child starts is the one thing that
        # lets the next child bind the path of the session.
        Path(remote).unlink()
        third = Bridge(self.root, [(remote, str(here))])
        self.addCleanup(third.stop)
        wait_for('the socket of the restarted child', third.binds)


class MachineRecordOfTheFixture(unittest.TestCase):
    """The record that the fixture writes says what the reader in core reads."""

    def test_the_machine_record_marks_the_cards_of_that_machine(self):
        with tempfile.TemporaryDirectory(prefix='hz-machine-record-') as folder:
            written = json.loads(serve.machine_watch_record(folder).read_text(encoding='utf-8'))
        record = written['machines'][0]
        self.assertIs(record.get(serve.MACHINE_RECORD_CARDS), True,
                      'the record that the fixture writes marks the cards of its machine')
        # The mark is a field of the record that the candidate reads. A fixture that
        # writes another name seeds a record with no card socket, so the agent of the
        # machine waits for ever and no card is drawn.
        reader = (ROOT / 'crates/horizon-core/src/machines/record.rs').read_text(encoding='utf-8')
        self.assertTrue(f'pub {serve.MACHINE_RECORD_CARDS}: bool,' in reader,
                        'the record in core declares the mark that the fixture writes')


class StandInSshTest(unittest.TestCase):
    """The stand-in `ssh` of the fixture reads the forwards of the child of a machine."""

    def test_the_stand_in_starts_the_bridge_for_a_forward_and_no_remote_command(self):
        with tempfile.TemporaryDirectory(prefix='hz-standin-') as folder:
            state = Path(folder)
            bin_folder = serve.machine_watch_standin(str(state))
            program = Path(bin_folder) / serve.MACHINE_WATCH_PROGRAM
            body = program.read_text()
            self.assertIn('-R)', body)
            self.assertIn(serve.MACHINE_CARD_BRIDGE, body)
            self.assertTrue((state / serve.MACHINE_WATCH_FOLDER / 'bin' / serve.MACHINE_CARD_BRIDGE).exists())

    def test_the_stand_in_runs_a_remote_command_of_a_panel(self):
        with tempfile.TemporaryDirectory(prefix='hz-standin-') as folder:
            state = Path(folder)
            bin_folder = serve.machine_watch_standin(str(state))
            program = Path(bin_folder) / serve.MACHINE_WATCH_PROGRAM
            answer = subprocess.run([str(program), 'sh', '-c', 'printf yes'],
                                    capture_output=True, text=True, timeout=30)
            self.assertEqual(answer.stdout, 'yes', answer.stderr)

    def test_a_machine_name_outside_the_rule_is_no_path(self):
        # The agent is the only program that names the socket of its session, and it
        # takes the path of the fixture and never a name of a report.
        with tempfile.TemporaryDirectory(prefix='hz-standin-') as folder:
            path = serve.machine_card_socket(folder, 'hz-night')
            self.assertTrue(str(path).endswith('/horizon-cards/build-1/hz-night.sock'), path)


# The relay of the relay lane: this file, run with one argument, answers the calls of
# its agent itself, and names the calls in the file that `FAKE_RELAY_LOG` names. The
# second `next_click` call is answered with the error of a lost link.
FAKE_RELAY_ANSWERS = [
    {'status': 'created', 'panel_id': 'card-relay', 'revision': 1},
    {'error': {'code': -32000, 'message': LOST}},
    CLICK_ANSWER,
    {'status': 'updated', 'panel_id': 'card-relay', 'revision': 2},
]


def serve_fake_relay() -> int:
    """Answers the calls of one agent, one error among them, and ends with its input."""
    log_path = os.environ.get('FAKE_RELAY_LOG')
    answers = iter(FAKE_RELAY_ANSWERS)
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
            answer = next(answers, None)
            if answer is None:
                continue
            if 'error' in answer:
                said = {'jsonrpc': '2.0', 'id': message['id'], 'error': answer['error']}
            else:
                said = {'jsonrpc': '2.0', 'id': message['id'],
                        'result': {'structuredContent': answer}}
            print(json.dumps(said), flush=True)
    finally:
        if log:
            log.close()
    return 0


if __name__ == '__main__':
    if len(sys.argv) == 2:
        # The agent of the relay lane runs this file as its relay program.
        sys.exit(serve_fake_relay())
    unittest.main()
