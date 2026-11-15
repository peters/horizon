#!/usr/bin/env python3
"""A stand-in pi panel that reports the state of its turn, so the sidebar words show.

Horizon starts a pi panel with `-e <report>` and the five `HORIZON_WORK_*`
variables of that one process. The report is the JavaScript module that Horizon
writes into its own home; it sends the lifecycle events of pi to
`<HORIZON_WORK_EXECUTABLE> --agent-work-hook`, which keeps the record that the
sidebar reads.

This script is the program of such a panel without pi. It takes the `-e` file of
its launch and runs the real report through `node` with a small `pi` object that
fires one event for each line that the person types. A computer without `node`
sends the same documents itself, so the record and the sidebar can be checked
there too; the extension itself is checked by the JavaScript test of this file's
harness.

One line for each event, and the event it fires:

  start   input, before_agent_start, turn_start   -> Working
  ask     ui_prompt_start                         -> Needs you
  answer  ui_prompt_end                           -> Working
  stop    agent_settled {aborted:false}           -> Done
  esc     agent_settled {aborted:true}            -> Done, interrupted
  quit    session_shutdown {reason:"quit"}         -> no claim

Standard library only, Python 3.10+, no network.
"""
from __future__ import annotations

import json
import os
import pathlib
import shutil
import subprocess
import sys

PROGRAM = 'pi-attention-stand-in'
HARNESS = pathlib.Path(__file__).with_name('pi_extension_harness.mjs')
TRANSCRIPT_NAME = 'pi-stand-in-session.jsonl'
ENTRY_ARGUMENT = '--agent-work-hook'
# The five variables of a launch with a report, and the identity that Horizon
# registered before this process started.
WORK_VARIABLES = (
    'HORIZON_WORK_ROOT',
    'HORIZON_WORK_PANEL',
    'HORIZON_WORK_OWNER',
    'HORIZON_WORK_KIND',
    'HORIZON_WORK_EXECUTABLE',
)
WORDS = ('start', 'ask', 'answer', 'stop', 'esc', 'quit')
# The events that each word fires, in the order that pi fires them. The fallback
# of a computer without `node` sends the documents of these events.
EVENTS = {
    'start': (('input',), ('before_agent_start',), ('turn_start',)),
    'ask': (('ui_prompt_start',),),
    'answer': (('ui_prompt_end',),),
    'stop': (('agent_settled',),),
    'esc': (('agent_settled',),),
    'quit': (('session_shutdown',),),
}


def report_file(argv):
    """The report that the launch named with `-e`, and that must be a file."""
    for index, argument in enumerate(argv[:-1]):
        if argument == '-e':
            return argv[index + 1]
    return None


def session_identity():
    """The identity of this process, as the report of a pi panel names itself."""
    return f'pi-process-{os.getpid()}'


def send(document):
    """Give one event to the entry point of the work hooks, in this environment."""
    command = [os.environ['HORIZON_WORK_EXECUTABLE'], ENTRY_ARGUMENT]
    finished = subprocess.run(
        command,
        input=json.dumps(document).encode('utf-8'),
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
    if finished.returncode != 0:
        detail = finished.stderr.decode('utf-8', 'replace').strip().splitlines()
        reason = f': {detail[-1]}' if detail else ''
        return f'event refused ({finished.returncode}){reason}'
    return None


class Fallback:
    """The documents of the report, for a computer that has no `node`.

    The report itself is the source of these documents: it starts the same entry
    with the same fields. This copy exists so that a test of the record and of the
    sidebar runs without Node; the extension is checked by the JavaScript test.
    """

    def __init__(self):
        self.session = session_identity()
        self.turns = 0
        self.prompt_id = None
        self.prompt_span = None

    def ensure_turn(self):
        if self.prompt_id is None:
            self.turns += 1
            self.prompt_id = f'turn-{os.getpid()}-{self.turns}'
            self.prompt_span = f'ui-prompt-{os.getpid()}-{self.turns}'
            return self.prompt_id, True
        return self.prompt_id, False

    def document(self, event, fields):
        document = {
            'session_id': self.session,
            'prompt_id': fields.get('prompt_id'),
            'hook_event_name': event,
            'cwd': os.getcwd(),
            'transcript_path': os.path.join(os.getcwd(), TRANSCRIPT_NAME),
        }
        if 'tool_use_id' in fields:
            document['tool_use_id'] = fields['tool_use_id']
        if 'reason' in fields:
            document['reason'] = fields['reason']
        return document

    def fire(self, word):
        if word in ('stop', 'esc'):
            prompt_id, self.prompt_id = self.prompt_id, None
            name = 'Interrupt' if word == 'esc' else 'Stop'
            return send(self.document(name, {'prompt_id': prompt_id}))
        if word == 'quit':
            self.prompt_id = None
            return send(self.document('SessionEnd', {'reason': 'quit'}))
        prompt_id, opened = self.ensure_turn()
        if word == 'ask':
            return send(self.document('PermissionRequest', {
                'prompt_id': prompt_id, 'tool_use_id': self.prompt_span}))
        if word == 'answer':
            return send(self.document('PostToolUse', {
                'prompt_id': prompt_id, 'tool_use_id': self.prompt_span}))
        # `start` opens the turn; the opening event is the one that claims work.
        return send(self.document('UserPromptSubmit', {'prompt_id': prompt_id})) if opened else None


class Harness:
    """The real report, run by `node` with the harness of this directory."""

    def __init__(self, extension):
        self.child = subprocess.Popen(
            ['node', str(HARNESS), '--extension', extension, '--session-dir', os.getcwd()],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)

    def fire(self, word):
        assert self.child.stdin is not None and self.child.stdout is not None
        self.child.stdin.write(f'{word}\n')
        self.child.stdin.flush()
        return self.child.stdout.readline().strip()

    def banner(self):
        assert self.child.stdout is not None
        return self.child.stdout.readline().strip()

    def close(self):
        assert self.child.stdin is not None
        self.child.stdin.close()
        self.child.wait(timeout=30)


def main(argv=None):
    argv = list(sys.argv[1:] if argv is None else argv)
    missing = [name for name in WORK_VARIABLES if not os.environ.get(name)]
    extension = report_file(argv)
    if missing:
        print(f'{PROGRAM}: {", ".join(missing)} is not set: enable the work report of the panel, '
              f'so Horizon attaches the record and the report of this launch', flush=True)
    elif extension is None or not os.path.isfile(extension):
        print(f'{PROGRAM}: the launch named no report file with -e: {extension!r}', flush=True)
    sender = None
    if not missing and extension is not None and os.path.isfile(extension):
        if shutil.which('node') is not None:
            sender = Harness(extension)
            print(f'{PROGRAM}: the report runs under node; {sender.banner()}', flush=True)
        else:
            sender = Fallback()
            print(f'{PROGRAM}: no node on PATH, so this stand-in sends the documents of the report; '
                  f'type one of {", ".join(WORDS)}', flush=True)
    for line in sys.stdin:
        word = line.strip().lower()
        if not word:
            continue
        if sender is None:
            print(f'{PROGRAM}: {word} fired nothing, because this launch has no report', flush=True)
            if word == 'quit':
                return 0
            continue
        if word == 'quit':
            answer = sender.fire('quit')
            if isinstance(sender, Harness):
                print(f'{PROGRAM}: {answer}', flush=True)
                sender.close()
            print(f'{PROGRAM}: quit fired', flush=True)
            return 0
        if word not in WORDS:
            print(f'{PROGRAM}: unknown word {word!r}; use {", ".join(WORDS)}', flush=True)
            continue
        answer = sender.fire(word)
        if isinstance(sender, Harness):
            print(f'{PROGRAM}: {answer}', flush=True)
        else:
            print(f'{PROGRAM}: {word} {answer or "sent"}', flush=True)
    return 0


if __name__ == '__main__':
    sys.exit(main())
