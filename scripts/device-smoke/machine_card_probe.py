#!/usr/bin/env python3
"""A probe of the card link of a machine: every call of one agent is answered.

It starts the relay program of the machines skill the way an agent program of the
machine starts it, says `initialize`, and calls `block_panel list` again and again,
with one pause between two calls. A cut of the link ends no call: the relay answers
the call that the cut reached with an error and meets the socket of its session
again. A run that reads one answer for every call therefore proves that Horizon
answers the next call after a cut of the link.

Run it in a `tmux` session of the machine, because the relay learns the name of its
session from `tmux`:

    python3 machine_card_probe.py --machine build-1 \
      --relay ~/.horizon-cards/relay.py --calls 10 --pause 4

The exit status is 0 when every call got its answer, and 1 when a call got no answer
or an error. Standard library only.
"""
from __future__ import annotations

import argparse
import json
import queue
import subprocess
import sys
import threading
import time

# The longest wait for one answer, in seconds. The relay waits at most 30 seconds for
# the socket of its session and then answers the call with the error of a lost link.
ANSWER_TIMEOUT_SECONDS = 60.0


def log(line: str) -> None:
    """One line of the probe, which the person reads in the pane of its session."""
    print(f'machine-card-probe: {line}', flush=True)


class Answers(threading.Thread):
    """The answers of the relay, which the probe reads while its call waits."""

    def __init__(self, stream):
        super().__init__(daemon=True)
        self.stream = stream
        self.lines: queue.Queue = queue.Queue()
        self.start()

    def run(self) -> None:
        for line in self.stream:
            self.lines.put(line)
        # The end of the relay is no answer: the probe reads it at once instead of
        # waiting out the bound of a call that no socket answers.
        self.lines.put(None)

    def hear(self) -> dict | None:
        """One answer of the relay, or `None` when no answer comes in time."""
        try:
            line = self.lines.get(timeout=ANSWER_TIMEOUT_SECONDS)
        except queue.Empty:
            return None
        return json.loads(line) if line is not None else None


def say(child: subprocess.Popen, message: dict) -> None:
    """One line to the relay, as the agent program writes it."""
    child.stdin.write(json.dumps(message) + '\n')
    child.stdin.flush()


def hello(child: subprocess.Popen, answers: Answers) -> bool:
    """Says `initialize` to the relay and reads its answer."""
    say(child, {'jsonrpc': '2.0', 'id': 1, 'method': 'initialize', 'params': {
        'protocolVersion': '2025-06-18', 'capabilities': {},
        'clientInfo': {'name': 'machine-card-probe', 'version': '1'}}})
    answer = answers.hear()
    if answer is None or 'error' in answer or answer.get('id') != 1:
        log(f'the relay answered no initialize: {answer}')
        return False
    say(child, {'jsonrpc': '2.0', 'method': 'notifications/initialized'})
    return True


def call(child: subprocess.Popen, answers: Answers, number: int, pause: float) -> bool:
    """One call of the probe, which says whether it got its answer."""
    if pause:
        time.sleep(pause)
    say(child, {'jsonrpc': '2.0', 'id': number, 'method': 'tools/call', 'params': {
        'name': 'block_panel', 'arguments': {'operation': 'list'}}})
    answer = answers.hear()
    if answer is None:
        log(f'call {number} got no answer')
    elif 'error' in answer:
        log(f'call {number} was answered with an error: '
            f'{(answer.get("error") or {}).get("message", "")}')
    elif answer.get('id') != number:
        log(f'call {number} was answered with the id of another call: {answer.get("id")}')
    else:
        log(f'call {number} is answered')
        return True
    return False


def probe(child: subprocess.Popen, answers: Answers, calls: int, pause: float) -> int:
    """Calls the card socket `calls` times, and says whether every call was answered."""
    if not hello(child, answers):
        return 1
    answered = sum(call(child, answers, number, pause if number > 2 else 0.0)
                   for number in range(2, calls + 2))
    log(f'{answered} of {calls} calls are answered; status {int(answered != calls)}')
    return 0 if answered == calls else 1


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description='Calls the card socket of a session through its relay again and again.')
    parser.add_argument('--machine', required=True,
                        help='the name of the machine, as its record names it')
    parser.add_argument('--relay', required=True,
                        help='the relay program of that machine, as the entry of its agent names it')
    parser.add_argument('--calls', type=int, default=10, help='how many calls the probe makes')
    parser.add_argument('--pause', type=float, default=4.0,
                        help='how long the probe waits between two calls, in seconds')
    arguments = parser.parse_args(argv)
    child = subprocess.Popen([sys.executable, '-u', arguments.relay, arguments.machine],
                             stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    answers = Answers(child.stdout)
    try:
        return probe(child, answers, arguments.calls, arguments.pause)
    except OSError as error:
        log(f'the relay of {arguments.machine} ended: {error}')
        return 1
    finally:
        try:
            child.stdin.close()
        except OSError:
            pass
        try:
            child.wait(timeout=10)
        except subprocess.TimeoutExpired:
            child.kill()


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
