#!/usr/bin/env python3
"""A stand-in agent that sends lifecycle events, so the attention states show.

The sidebar says which agent panel needs the person, which one finished while
the person looked elsewhere and which one works. A panel whose launch registered
a work ledger takes that state from the ledger, and the lifecycle hooks of the
agent keep the ledger current. This script is the program of such a panel
without any real agent: it sends the same events through the same entry point
that the installed hooks use, `<candidate> --agent-work-hook`, with the
environment that a hooking launch sets.

One event for each line that the person types into the panel:

  start       a turn starts                                 -> Working
  ask         the agent waits for permission                -> Needs you
  answer      the person answered                           -> Working
  stop        the turn ends                                 -> Done
  background  the turn ends and background work still runs  -> Works in the background
  failed      the turn failed                               -> Needs you
  quit        the script ends

`--timer SECONDS` sends the same events in a sequence of its own, one every
SECONDS, so a procedure can watch the states of the sidebar without typing.

The fixture starts the script. `--register` writes the work record and the
health marker of the launch, in the shape that
`crates/horizon-core/src/agent_work/store.rs` reads, and ends. It writes no
prompt and no tool content: an event carries the session, the turn, the event
name, a tool identity and, for a stop, how much work still runs in the
background.

Standard library only, Python 3.10+, no network.
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time

PROGRAM = 'attention-stand-in'

# `SCHEMA_VERSION` of crates/horizon-core/src/agent_work/store.rs.
RECORD_VERSION = 1
# The words of the script that name one lifecycle event, and the event each one
# sends. The turn identity is the one of the last `start`.
EVENTS = {
    'start': {'hook_event_name': 'UserPromptSubmit'},
    'ask': {'hook_event_name': 'PermissionRequest', 'tool_use_id': 'tool-1', 'tool_name': 'Bash'},
    'answer': {'hook_event_name': 'PostToolUse', 'tool_use_id': 'tool-1', 'tool_name': 'Bash'},
    'stop': {'hook_event_name': 'Stop'},
    'background': {'hook_event_name': 'Stop', 'background_tasks': 2},
    'failed': {'hook_event_name': 'StopFailure'},
}
# The sequence of `--timer`, in the order that shows every state.
TIMER_SEQUENCE = ('start', 'stop', 'start', 'background', 'start', 'failed', 'start', 'ask', 'answer', 'stop')
SESSION_ID = 'stand-in-session'
MIN_TIMER_SECONDS = 0.2


def safe_local_id(local_id):
    """The path component of a panel id: `%` and the hex of every byte.

    `horizon_browser_control::paths::safe_local_id` writes the same string, so a
    record written here is the record that a launch of the panel would write.
    """
    return '%' + ''.join(f'{byte:02x}' for byte in local_id.encode('utf-8'))


def record_document(config):
    """The `StoredWork` of an agent launch, with a ledger that no event touched yet."""
    return {
        'version': RECORD_VERSION,
        'panel_local_id': config['panel'],
        'kind': config['kind'],
        'owner_token': config['owner'],
        'cwd': config['cwd'],
        'transcript_path': '',
        'ledger': {},
        'handoff': None,
        'consumed': False,
    }


def register(config):
    """Write the work record and the health marker of this launch.

    The store reads a record that this home registered, and it takes no event of
    a launch whose health marker is missing or was invalidated.
    """
    work = os.path.join(config['root'], 'agent-work')
    panel = safe_local_id(config['panel'])
    record = os.path.join(work, f'{panel}.json')
    health = os.path.join(work, 'health', panel, config['owner'])
    for path in (work, os.path.dirname(record), os.path.dirname(health)):
        os.makedirs(path, mode=0o700, exist_ok=True)
    with open(health, 'w', encoding='utf-8') as marker:
        marker.write('1')
    os.chmod(health, 0o600)
    with open(record, 'w', encoding='utf-8') as document:
        json.dump(record_document(config), document)
    os.chmod(record, 0o600)
    return record


def event_document(config, word, prompt_id):
    """One normalized lifecycle event, without a prompt or a tool content."""
    event = {'session_id': SESSION_ID, 'prompt_id': prompt_id, 'cwd': config['cwd'],
             'transcript_path': config['config']}
    event.update(EVENTS[word])
    return event


def hook_command(config):
    """The entry point that takes one event: the real one, or a test's own."""
    return config.get('hook') or [config['exe'], '--agent-work-hook']


def send(config, document):
    """Give one event to the entry point of the installed hooks."""
    environment = dict(os.environ)
    environment.update({
        'HORIZON_WORK_ROOT': config['root'],
        'HORIZON_WORK_PANEL': config['panel'],
        'HORIZON_WORK_OWNER': config['owner'],
        'HORIZON_WORK_KIND': config['kind'],
        'HORIZON_WORK_EXECUTABLE': config['exe'],
    })
    finished = subprocess.run(
        hook_command(config),
        input=json.dumps(document).encode('utf-8'),
        env=environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
    if finished.returncode != 0:
        return f'event refused ({finished.returncode})'
    return None


class StandIn:
    """The turn identity that one event of the script needs."""

    def __init__(self, config):
        self.config = config
        self.turns = 0
        self.prompt_id = 'turn-0'

    def send(self, word):
        if word not in EVENTS:
            return f'unknown event {word!r}; use one of {", ".join(EVENTS)}'
        if word == 'start':
            self.turns += 1
            self.prompt_id = f'turn-{self.turns}'
        refused = send(self.config, event_document(self.config, word, self.prompt_id))
        return refused or f'{word}: {EVENTS[word]["hook_event_name"]}'


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--config', required=True, help='JSON document of this stand-in panel')
    parser.add_argument('--register', action='store_true', help='Write the work record and health marker, then end')
    parser.add_argument('--timer', type=float, default=None,
                        help='Send the events in a sequence, one every SECONDS, instead of waiting for lines')
    arguments, _unknown = parser.parse_known_args()
    with open(arguments.config, encoding='utf-8') as document:
        config = json.load(document)
    if arguments.register:
        register(config)
        print(f'{PROGRAM}: registered the work ledger of {config["panel"]}', flush=True)
        return 0
    stand_in = StandIn(config)
    if arguments.timer is not None:
        return run_timer(stand_in, arguments.timer)
    print(f'{PROGRAM}: type one of {", ".join(EVENTS)} and press Enter', flush=True)
    for line in sys.stdin:
        word = line.strip().lower()
        if not word:
            continue
        if word == 'quit':
            return 0
        print(f'{PROGRAM}: {stand_in.send(word)}', flush=True)
    return 0


def run_timer(stand_in, seconds):
    """Sends the sequence of [`TIMER_SEQUENCE`] until the process ends."""
    interval = max(seconds, MIN_TIMER_SECONDS)
    while True:
        for word in TIMER_SEQUENCE:
            print(f'{PROGRAM}: {stand_in.send(word)}', flush=True)
            time.sleep(interval)


if __name__ == '__main__':
    sys.exit(main())
