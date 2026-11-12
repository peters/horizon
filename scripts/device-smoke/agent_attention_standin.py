#!/usr/bin/env python3
"""A stand-in agent that sends the lifecycle events of a turn, so the attention states show.

The sidebar says which agent panel needs the person, which one finished while
the person looked elsewhere and which one works. A panel whose launch attached a
work ledger takes that state from the ledger, and the lifecycle hooks of the
agent keep the ledger current. This script is the program of such a panel
without any real agent: it sends the same events through the same entry point
that the installed hooks use, `<HORIZON_WORK_EXECUTABLE> --agent-work-hook`, and
it takes the identity of its launch from the environment that Horizon set.

Horizon is the only source of that identity, so this program registers nothing.
It ignores every argument that Horizon gives the panel of an agent, and it uses
the session identity that the launch names.

One event for each line that the person types into the panel:

  start       a turn starts                                 -> Working
  ask         the agent waits for permission                -> Needs you
  answer      the person answered                           -> Working
  stop        the turn ends                                 -> Done
  background  the turn ends and background work still runs  -> Background
  failed      the turn failed                               -> Needs you
  quit        the script ends

A notification arrives only while no window of Horizon has the focus, and a
keypress gives Horizon the focus again. `after SECONDS WORD` therefore sends
`WORD` after that delay, so the person can type the line, give the focus to
another window, and still receive the event. The delay is at most 60 seconds.

`--timer SECONDS` sends the same events in a sequence of its own, one turn every
SECONDS, and the stand-in still reads the lines of the person while it runs. The
fixture passes the timer in the JSON file that the stand-in reads beside itself.

Standard library only, Python 3.10+, no network.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import threading
import time

PROGRAM = 'attention-stand-in'
# The document that the fixture writes beside the stand-in: the timer of this
# run, and nothing that Horizon owns.
CONFIG_NAME = 'stand-in.json'
# The environment of the launch. Horizon sets all five when it attaches the work
# ledger of an agent panel, and the hook entry reads them back.
WORK_VARIABLES = (
    'HORIZON_WORK_ROOT',
    'HORIZON_WORK_PANEL',
    'HORIZON_WORK_OWNER',
    'HORIZON_WORK_KIND',
    'HORIZON_WORK_EXECUTABLE',
)
# The arguments of a Claude panel that name the conversation of this launch.
SESSION_ARGUMENTS = ('--session-id', '--resume')
DEFAULT_SESSION = 'stand-in-session'
TRANSCRIPT_NAME = 'stand-in-transcript.jsonl'
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
MIN_TIMER_SECONDS = 0.2
# The longest delay of `after`, in seconds: a person waits for an event and not
# for minutes.
MAX_AFTER_SECONDS = 60


class Options:
    """What this run was asked for: the document beside the stand-in, and the timer."""

    def __init__(self, config=None, timer=None):
        self.config = config
        self.timer = timer


def parse_arguments(argv):
    """Read the two arguments of this stand-in, and ignore every argument of Horizon.

    Horizon starts the panel of an agent with its own arguments: `--plugin-dir`
    any number of times, `--session-id` or `--resume`, `--continue`, and others.
    An argument that this program does not know is not an error, so a launch of
    Horizon never ends because of one.
    """
    options = Options()
    index = 0
    while index < len(argv):
        argument = argv[index]
        value = argv[index + 1] if index + 1 < len(argv) else None
        if argument == '--config' and value is not None:
            options.config = value
            index += 2
        elif argument.startswith('--config='):
            options.config = argument.split('=', 1)[1]
            index += 1
        elif argument == '--timer' and value is not None:
            options.timer = value
            index += 2
        elif argument.startswith('--timer='):
            options.timer = argument.split('=', 1)[1]
            index += 1
        else:
            index += 1
    return options


def config_path(options):
    """The document of this run: the one it names, else the one beside the stand-in."""
    if options.config:
        return options.config
    return os.path.join(os.path.dirname(os.path.abspath(__file__)), CONFIG_NAME)


def read_config(options):
    """The document of this run, or an empty one when the fixture wrote none."""
    try:
        with open(config_path(options), encoding='utf-8') as document:
            value = json.load(document)
    except FileNotFoundError:
        return {}
    return value if isinstance(value, dict) else {}


def missing_variables():
    """The work variables that Horizon did not set for this launch."""
    return [name for name in WORK_VARIABLES if not os.environ.get(name)]


def launch_session(argv):
    """The conversation that this launch names, from `--session-id` or `--resume`."""
    session = DEFAULT_SESSION
    for index, argument in enumerate(argv[:-1]):
        if argument in SESSION_ARGUMENTS and argv[index + 1]:
            session = argv[index + 1]
    return session


def event_document(word, prompt_id, session_id):
    """One normalized lifecycle event, without a prompt or a tool content."""
    cwd = os.getcwd()
    event = {'session_id': session_id, 'prompt_id': prompt_id, 'cwd': cwd,
             'transcript_path': os.path.join(cwd, TRANSCRIPT_NAME)}
    event.update(EVENTS[word])
    return event


def send(document):
    """Give one event to the entry point of the installed hooks, in this environment."""
    command = [os.environ['HORIZON_WORK_EXECUTABLE'], '--agent-work-hook']
    finished = subprocess.run(
        command,
        input=json.dumps(document).encode('utf-8'),
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
    if finished.returncode != 0:
        detail = finished.stderr.decode('utf-8', 'replace').strip().splitlines()
        reason = f': {detail[-1]}' if detail else ''
        return f'event refused ({finished.returncode}){reason}'
    return None


class StandIn:
    """The turn identity that one event of the script needs."""

    def __init__(self, session_id, missing):
        self.session_id = session_id
        self.missing = missing
        self.turns = 0
        self.prompt_id = 'turn-0'
        # The timer and the lines of the person send at the same time, so the
        # turn identity follows one event at a time.
        self.lock = threading.Lock()

    def send(self, word):
        if word not in EVENTS:
            return f'unknown event {word!r}; use one of {", ".join(EVENTS)}'
        if self.missing:
            return f'{word}: no event is sent, because Horizon set no work identity'
        with self.lock:
            if word == 'start':
                self.turns += 1
                self.prompt_id = f'turn-{self.turns}'
            document = event_document(word, self.prompt_id, self.session_id)
        refused = send(document)
        return refused or f'{word}: {EVENTS[word]["hook_event_name"]}'


def run_timer(stand_in, seconds):
    """Sends the sequence of [`TIMER_SEQUENCE`] until the process ends."""
    interval = max(seconds, MIN_TIMER_SECONDS)
    while True:
        for word in TIMER_SEQUENCE:
            print(f'{PROGRAM}: {stand_in.send(word)}', flush=True)
            time.sleep(interval)


def parse_delay(text):
    """The delay and the word of `after SECONDS WORD`, or `None` for another line.

    Raises `ValueError` with a sentence for the panel when the line is a delay that
    this stand-in cannot send.
    """
    parts = text.split()
    if not parts or parts[0] != 'after':
        return None
    if len(parts) != 3:
        raise ValueError(f'{text!r}: write "after SECONDS WORD"')
    try:
        seconds = float(parts[1])
    except ValueError:
        raise ValueError(f'{parts[1]!r} is no number of seconds') from None
    if not 0 <= seconds <= MAX_AFTER_SECONDS:
        raise ValueError(f'{seconds:g} s is outside 0 to {MAX_AFTER_SECONDS} s')
    if parts[2] not in EVENTS:
        raise ValueError(f'unknown event {parts[2]!r}; use one of {", ".join(EVENTS)}')
    return seconds, parts[2]


def send_later(stand_in, seconds, word):
    """Sends `word` after `seconds`, so the person can move the focus away first."""
    time.sleep(seconds)
    print(f'{PROGRAM}: {stand_in.send(word)}', flush=True)


def main(argv=None):
    argv = list(sys.argv[1:] if argv is None else argv)
    options = parse_arguments(argv)
    config = read_config(options)
    missing = missing_variables()
    stand_in = StandIn(launch_session(argv), missing)
    if missing:
        print(f'{PROGRAM}: {", ".join(missing)} is not set: set "work_resume": {{"enabled": true}} '
              f'in scripts/device-smoke/attention_panel.json, so Horizon attaches the work ledger of this panel',
              flush=True)
    else:
        print(f'{PROGRAM}: type one of {", ".join(EVENTS)}, or "after SECONDS WORD", and press Enter', flush=True)
    timer = options.timer if options.timer is not None else config.get('timer')
    if timer:
        # The timer is a second sender of the same events, and the person still
        # types into the panel while it runs.
        threading.Thread(target=run_timer, args=(stand_in, float(timer)), daemon=True).start()
    pending = []
    for line in sys.stdin:
        text = line.strip().lower()
        if not text:
            continue
        if text == 'quit':
            return 0
        try:
            delay = parse_delay(text)
        except ValueError as error:
            print(f'{PROGRAM}: {error}', flush=True)
            continue
        if delay is None:
            print(f'{PROGRAM}: {stand_in.send(text)}', flush=True)
            continue
        seconds, word = delay
        later = threading.Timer(seconds, send_later, args=(stand_in, seconds, word))
        # A delay must not hold the panel after `quit`, so the thread ends with this
        # process.
        later.daemon = True
        later.start()
        pending.append(later)
        print(f'{PROGRAM}: {word} in {seconds:g} s', flush=True)
    # A delay outlives the typed lines: the panel ends after the last one sends. Every
    # delay was scheduled at most this long ago, so one deadline ends them all.
    deadline = time.monotonic() + MAX_AFTER_SECONDS + 1
    for later in pending:
        later.join(max(deadline - time.monotonic(), 0))
    return 0


if __name__ == '__main__':
    sys.exit(main())
