#!/usr/bin/env python3
"""A stand-in recorder for speech input: copy a prepared WAV and wait for SIGINT.

The fixture of a speech test names this program in the settings as the recorder,
so a run needs no microphone and opens no audio device of the computer:

    /usr/bin/python3 -B scripts/device-smoke/speech_standin_recorder.py {file} \\
        --source /tmp/take.wav

The program copies `--source` to the take file of the settings and then waits for
`SIGINT`, which is the signal that Horizon sends to the exact child when the
hotkey comes up. `--seconds` bounds the wait, so a run that never sends the signal
leaves nothing behind. The program writes no other file and prints nothing.
"""
from __future__ import annotations

import argparse
from pathlib import Path
import shutil
import signal
import sys
import threading

# The longest time that a stand-in waits, when the caller gives none.
DEFAULT_SECONDS = 3600.0


def parse_arguments(argv):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('take', type=Path, help='The file that the settings replace {file} with')
    parser.add_argument('--source', type=Path, required=True, help='The prepared WAV to copy')
    parser.add_argument('--seconds', type=float, default=DEFAULT_SECONDS,
                        help='The longest time to wait for SIGINT')
    return parser.parse_args(argv)


def copy_and_wait(take, source, seconds=DEFAULT_SECONDS):
    """Copy `source` to `take` and wait for SIGINT for at most `seconds`.

    Answers the text that the recorder wrote, which is empty: a stand-in writes the
    file and nothing else. A signal that arrives before the wait starts still ends
    it, because the stop flag is set before the copy.
    """
    stopped = threading.Event()

    def stop(_signum, _frame):
        stopped.set()

    previous_int = signal.signal(signal.SIGINT, stop)
    previous_term = signal.signal(signal.SIGTERM, stop)
    try:
        take.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, take)
        stopped.wait(max(0.0, seconds))
    finally:
        signal.signal(signal.SIGINT, previous_int)
        signal.signal(signal.SIGTERM, previous_term)
    return ''


def main(argv):
    args = parse_arguments(argv)
    if not args.source.is_file():
        print(f'{args.source} is not a file', file=sys.stderr)
        return 2
    copy_and_wait(args.take, args.source, args.seconds)
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
