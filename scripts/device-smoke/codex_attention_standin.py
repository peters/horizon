#!/usr/bin/env python3
"""A stand-in codex panel that writes the state of its turn into its own title.

Codex writes its state into the title of the terminal that runs it, with no
configuration of its own, and it also reports it with the Program Status
Protocol (OSC 7501):

  * while it works, a braille spinner frame and the name of the folder, for
    example `⠙ work`, and a new frame about ten times a second;
  * while an approval waits, two frames of a flagged line, for example
    `[ . ] Action Required | Create a file | work` and the same line with
    `[ ! ]`, about twice a second;
  * when nothing runs, a title with no frame and no `Action Required`;
  * beside every title, one report of the program status protocol, for example
    `ESC ] 7501 ; state=working:app=codex ST`, with `state=blocked` and
    `kind=permission` while an approval waits, `state=done` at the end of a turn,
    `state=idle` after an interruption and `state=clear` at the end.

This script is the program of such a panel without codex. It reads one word for
each line of its input and writes the real title sequences to its own terminal,
so a run of Horizon reads them through the reader of a panel. With `--protocol` it
writes the reports of the program status protocol too, as the real program does
with no configuration of its own. With `--startup` it writes the launch of the
real program before it reads a word: eight frames of the spinner a tenth of a
second apart, and then the plain title that says it is ready for the person
(with `--protocol`, one `state=working` report with each frame and one
`state=idle` at the moment it is ready). With `--startup-question` the launch
blinks a trust question and reports a blocked turn before it is ready, as the
real program asks before it shows its prompt:

  start   the spinner of a working turn               -> Working
  ask     the two frames of an approval               -> Needs you
  answer  the spinner of the same turn again          -> Working
  deny    a plain title and `state=idle`              -> the turn of an approval
                                                         that was denied is an
                                                         interrupted turn
  stop    a plain title                               -> Done while unseen
  esc     a plain title, as an interrupted turn       -> Done while unseen
  quit    a plain title, then the end of the program  -> no claim
  fail    a plain title and `state=error`             -> Needs you
  foreign `state=working:app=other`                   -> nothing changes

`spin N` writes `N` spinner frames of the turn at once, `work N` writes `N`
reports of `state=working` at once, and `plain SECONDS`
writes a plain title, waits, and writes the spinner of the same turn again, which
is the title of a program between two parts of one turn. `after SECONDS WORD`
sends `WORD` after that delay, so the person can move the focus away before the
turn ends. The delay is at most 60 seconds.

Standard library only, Python 3.9+, no network.
"""
from __future__ import annotations

import os
import sys
import threading
import time

PROGRAM = 'codex-attention-stand-in'
# The argument that makes this program write the reports of the program status
# protocol beside its titles, as the real program does with no configuration.
PROTOCOL_ARGUMENT = '--protocol'
# The frames of the spinner of a working turn, in the order that they change.
SPINNER_FRAMES = '\u280b\u2819\u2839\u2838\u283c\u2834\u2826\u2827\u2807\u280f'
# How often a working turn writes a new frame, in seconds.
FRAME_SECONDS = 0.1
# The two frames of a line that waits for an approval, and the pause between them.
WAITING_FRAMES = ('[ . ] Action Required', '[ ! ] Action Required')
WAITING_SECONDS = 0.25
# The words that a person types, and the longest delay of `after`.
WORDS = ('start', 'ask', 'answer', 'deny', 'stop', 'esc', 'quit', 'fail', 'foreign')
# The argument that makes this program write the launch of the real program
# before it reads a word: frames of the spinner while it loads, and then the plain
# title that says it is ready for the person.
STARTUP_ARGUMENT = '--startup'
# The argument that makes the launch of this program ask a question of the person
# before the program is ready, as a trust question of the real program does.
STARTUP_QUESTION_ARGUMENT = '--startup-question'
# The frames of the spinner of a launch, and the pause between two of them.
STARTUP_FRAMES = 8
STARTUP_SECONDS = 0.1
# How long the ready title of a launch stands before the program reads a word.
# The ready title of the real program stands while the program waits for the
# person, and a panel reads the title of its program once per frame: a title that a
# later one replaces within one frame is not read at all.
READY_SECONDS = 0.3
# How long a question of a launch blinks its two frames, in rounds of the two.
STARTUP_QUESTION_ROUNDS = 4
# The reports of the program status protocol that a word writes, in the bytes of
# the real program. `state=clear` carries no `app`, as the real program writes it.
REPORTS = {
    'start': ('state=working:app=codex',),
    'answer': ('state=working:app=codex',),
    'ask': ('state=blocked:app=codex:kind=permission',),
    'stop': ('state=done:app=codex', 'state=idle:app=codex'),
    'esc': ('state=idle:app=codex',),
    # A denial ends a turn that was stopped, so the program reports the state of
    # an interrupted turn and no done.
    'deny': ('state=idle:app=codex',),
    'fail': ('state=error:app=codex',),
    'foreign': ('state=working:app=other',),
    'quit': ('state=clear',),
}
MAX_AFTER_SECONDS = 60
MAX_FRAMES = 10_000


class Status:
    """The reports of the program status protocol of the panel."""

    def __init__(self, out=None, enabled=True):
        self.out = out if out is not None else sys.stdout
        self.enabled = enabled
        self.lock = threading.Lock()

    def report(self, payload):
        """Writes one report, in the bytes of the real program."""
        if not self.enabled:
            return
        with self.lock:
            self.out.write(f'\x1b]7501;{payload}\x1b\\')
            self.out.flush()


class Titles:
    """The title of the panel, in the forms that the real program writes."""

    def __init__(self, folder, out=None):
        self.folder = folder
        self.out = out if out is not None else sys.stdout
        self.lock = threading.Lock()
        self.frame = 0
        self.spinning = threading.Event()
        # One title is a whole sequence, so the frames of the spinner and the
        # lines of another state never mix.
        self.thread = threading.Thread(target=self._spin, daemon=True)
        self.thread.start()

    def write(self, title):
        """Writes one title of the panel, as the real program writes it."""
        with self.lock:
            self._write_locked(title)

    def working(self):
        """A working turn: the spinner starts with a frame of its own."""
        self.frame = 0
        with self.lock:
            self._write_locked(self.frame_title())
            self.spinning.set()

    def waiting(self, what='Create a file'):
        """An approval waits: the two frames of the line, as the program blinks."""
        self._stop_spinning()
        for frame in WAITING_FRAMES:
            self.write(f'{frame} | {what} | {self.folder}')
            time.sleep(WAITING_SECONDS)

    def plain(self):
        """A title that names no state: nothing runs."""
        self._stop_spinning()
        self.write(self.folder)

    def frame_title(self):
        """The title of one frame of the spinner of a working turn."""
        return f'{SPINNER_FRAMES[self.frame % len(SPINNER_FRAMES)]} {self.folder}'

    def step(self):
        """Writes the next frame of the spinner, as a loading program does."""
        self._stop_spinning()
        self.frame += 1
        self.write(self.frame_title())

    def frames(self, count):
        """Writes `count` spinner frames of the turn at once."""
        self._stop_spinning()
        for _ in range(count):
            self.frame += 1
            self.write(self.frame_title())

    def close(self):
        """Stops the spinner, so no frame follows the end of the program."""
        self._stop_spinning()

    def _write_locked(self, title):
        self.out.write('\x1b]0;' + title + '\x07')
        self.out.flush()

    def _stop_spinning(self):
        # The lock keeps a frame of the spinner out of a title of another state:
        # the thread below checks the flag under the same lock.
        with self.lock:
            self.spinning.clear()

    def _spin(self):
        # A working turn writes a new frame every FRAME_SECONDS. The check and the
        # write hold the lock, so no frame of the spinner follows a title of
        # another state.
        while True:
            time.sleep(FRAME_SECONDS)
            with self.lock:
                if not self.spinning.is_set():
                    continue
                self.frame += 1
                self._write_locked(self.frame_title())


def parse_delay(text):
    """The delay and the word of `after SECONDS WORD`, or `None` for another line."""
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
    if parts[2] not in WORDS:
        raise ValueError(f'unknown word {parts[2]!r}; use {", ".join(WORDS)}')
    return seconds, parts[2]


def parse_frames(text):
    """The count of `spin N` or of `work N`, or `None` for another line."""
    parts = text.split()
    if not parts or parts[0] not in ('spin', 'work'):
        return None
    if len(parts) != 2:
        raise ValueError(f'{text!r}: write "spin N" or "work N"')
    try:
        count = int(parts[1])
    except ValueError:
        raise ValueError(f'{parts[1]!r} is no count of frames') from None
    if not 1 <= count <= MAX_FRAMES:
        raise ValueError(f'{count} frames are outside 1 to {MAX_FRAMES}')
    return count


def parse_plain(text):
    """The delay of `plain SECONDS`, or `None` for another line."""
    parts = text.split()
    if not parts or parts[0] != 'plain':
        return None
    if len(parts) != 2:
        raise ValueError(f'{text!r}: write "plain SECONDS"')
    try:
        seconds = float(parts[1])
    except ValueError:
        raise ValueError(f'{parts[1]!r} is no number of seconds') from None
    if not 0 <= seconds <= MAX_AFTER_SECONDS:
        raise ValueError(f'{seconds:g} s is outside 0 to {MAX_AFTER_SECONDS} s')
    return seconds


def apply_word(titles, status, word):
    """Writes the titles and the reports of one word of the person."""
    reports = REPORTS.get(word)
    if reports is None:
        raise ValueError(f'unknown word {word!r}')
    if word in ('start', 'answer'):
        titles.working()
    elif word == 'ask':
        titles.waiting()
    elif word in ('foreign', 'quit'):
        pass
    else:
        # `stop`, `esc` and `deny`: the program writes a plain title and no frame.
        titles.plain()
    for payload in reports:
        status.report(payload)


def send_later(titles, status, word):
    """Writes the titles and the reports of `word` when its delay ends."""
    print(f'{PROGRAM}: {word} now', flush=True)
    apply_word(titles, status, word)


def writes_protocol(argv):
    """Whether this run writes the reports of the program status protocol too."""
    return any(argument == PROTOCOL_ARGUMENT for argument in argv)


def start_up(titles, status, question=False):
    """Writes the launch of the real program, before it reads a word.

    The real program writes a frame of its spinner about ten times a second while
    it loads, and then the plain title that says it is ready for the person. A
    launch that reports the program status protocol writes `state=working` with
    every frame and `state=idle` at the moment it is ready. A `question` is the
    trust question of the real program before it is ready: the two frames of the
    approval line, and one report of a blocked turn.
    """
    for _ in range(STARTUP_FRAMES):
        titles.step()
        status.report('state=working:app=codex')
        time.sleep(STARTUP_SECONDS)
    if question:
        for _ in range(STARTUP_QUESTION_ROUNDS):
            titles.waiting('Do you trust the files in this folder?')
        status.report('state=blocked:app=codex:kind=permission')
    status.report('state=idle:app=codex')
    titles.plain()
    time.sleep(READY_SECONDS)


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    folder = os.path.basename(os.getcwd()) or 'work'
    titles = Titles(folder)
    status = Status(enabled=writes_protocol(argv))
    print(
        f'{PROGRAM}: type one of {", ".join(WORDS)}; the title of this panel shows the state',
        flush=True,
    )
    if any(argument == STARTUP_ARGUMENT for argument in argv):
        start_up(titles, status, question=any(argument == STARTUP_QUESTION_ARGUMENT for argument in argv))
    pending = []
    for line in sys.stdin:
        text = line.strip().lower()
        if not text:
            continue
        if text == 'quit':
            titles.plain()
            apply_word(titles, status, 'quit')
            titles.close()
            print(f'{PROGRAM}: quit', flush=True)
            return 0
        try:
            delay = parse_delay(text)
            frames = parse_frames(text)
            plain_seconds = parse_plain(text)
        except ValueError as error:
            print(f'{PROGRAM}: {error}', flush=True)
            continue
        if delay is not None:
            seconds, word = delay
            later = threading.Timer(seconds, send_later, args=(titles, status, word))
            # A delay must not hold the panel after `quit`.
            later.daemon = True
            later.start()
            pending.append(later)
            print(f'{PROGRAM}: {word} in {seconds:g} s', flush=True)
            continue

        if frames is not None:
            if text.startswith('work'):
                for _ in range(frames):
                    status.report('state=working:app=codex')
            else:
                titles.frames(frames)
            print(f'{PROGRAM}: {frames} frames', flush=True)
            continue
        if plain_seconds is not None:
            titles.plain()
            time.sleep(plain_seconds)
            titles.working()
            print(f'{PROGRAM}: a plain title for {plain_seconds:g} s', flush=True)
            continue
        if text not in WORDS:
            print(f'{PROGRAM}: unknown word {text!r}; use {", ".join(WORDS)}', flush=True)
            continue
        apply_word(titles, status, text)
        print(f'{PROGRAM}: {text}', flush=True)
    # A delay outlives the typed lines: the panel ends after the last one sends.
    deadline = time.monotonic() + MAX_AFTER_SECONDS + 1
    for later in pending:
        later.join(max(deadline - time.monotonic(), 0))
    titles.close()
    return 0


if __name__ == '__main__':
    sys.exit(main())
