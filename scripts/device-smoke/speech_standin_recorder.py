#!/usr/bin/env python3
"""A stand-in recorder for speech input: copy a prepared WAV and wait for SIGINT.

The fixture of a speech test names this program in the settings as the recorder,
so a run needs no microphone and opens no audio device of the computer:

    /usr/bin/python3 -B scripts/device-smoke/speech_standin_recorder.py {file} \\
        --source /tmp/take.wav

The program copies `--source` to the take file of the settings and then waits for
`SIGINT`, which is the signal that Horizon sends to the exact child when the
hotkey comes up. `--open-header` leaves the two length fields of a canonical PCM
header at zero, which is the file that a recorder leaves when Horizon had to kill
it. `--seconds` bounds the wait, so a run that never sends the signal
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
    parser.add_argument('--open-header', action='store_true',
                        help='Leave the two length fields of a canonical PCM header at zero, '
                             'as a recorder that had to be killed does')
    return parser.parse_args(argv)


# The offset of the `RIFF` chunk size and of the `data` chunk size of a canonical
# `RIFF`/`WAVE` header of PCM audio, and the length of that header.
RIFF_SIZE_AT = 4
DATA_SIZE_AT = 40
CANONICAL_HEADER = 44


def open_header(take):
    """Write zero into the two length fields of a canonical PCM header.

    A recorder that Horizon had to kill never writes the length of its file, so the
    file of the take stays a canonical header with no length and the audio behind it.
    This writes the state that such a recorder leaves.
    """
    with take.open('r+b') as stream:
        content = stream.read()
        if len(content) < CANONICAL_HEADER or content[0:4] != b'RIFF' or content[8:12] != b'WAVE':
            return False
        for at in (RIFF_SIZE_AT, DATA_SIZE_AT):
            stream.seek(at)
            stream.write(b'\x00\x00\x00\x00')
    return True


def copy_and_wait(take, source, seconds=DEFAULT_SECONDS, leave_open=False):
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
        if leave_open:
            open_header(take)
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
    copy_and_wait(args.take, args.source, args.seconds, args.open_header)
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
