#!/usr/bin/env python3
"""A stand-in frame source program for the tests of Horizon.

This program speaks the picture and control contract of a frame source viewer and
nothing else: it is not a screen capture, it opens no device and it reads nothing of
the computer. Its pictures are a pattern that it draws itself, with the sequence
number of each picture in it, so a test sees that the newest picture arrived and that
the picture moved.

The contract, as this program uses it:

- Descriptor 3 is a Unix stream socket. A picture is a 32-byte header and then the
  planes: the bytes ``NV12FRM1``, the sequence number, the time of the capture in
  microseconds, the width and the height, each whole number least significant byte
  first, then the Y plane and the interleaved UV plane.
- Standard input and standard output carry control messages: four bytes of length,
  most significant byte first, and one JSON object with a member ``type``.

The program exits with 0 at the end of its standard input, at ``bye``, at SIGTERM
and when its socket closes. It exits with 1 with one line on its standard error when
its source fails, which ``--fail-after`` stands in for.

Usage:
  frame_source_standin.py --width 320 --height 180 --rate 10
  frame_source_standin.py --input state/input.jsonl --report state/report.json
"""

import argparse
import json
import os
import signal
import socket
import struct
import sys
import threading
import time

MAGIC = b'NV12FRM1'
SOCKET_FD = 3
MIN_SIDE = 16
MAX_SIDE = 8192
MAX_MESSAGE_BYTES = 65536

# A 3x5 font for the digits of the sequence number, so a test can read the number of
# the picture that it shows out of the picture itself.
DIGITS = {
    '0': ('111', '101', '101', '101', '111'),
    '1': ('010', '110', '010', '010', '111'),
    '2': ('111', '001', '111', '100', '111'),
    '3': ('111', '001', '111', '001', '111'),
    '4': ('101', '101', '111', '001', '001'),
    '5': ('111', '100', '111', '001', '111'),
    '6': ('111', '100', '111', '101', '111'),
    '7': ('111', '001', '001', '001', '001'),
    '8': ('111', '101', '111', '101', '111'),
    '9': ('111', '101', '111', '001', '111'),
}


class Stop(Exception):
    """The program ends with 0: its socket closed, its input ended or it was told."""


def even(value, smallest=MIN_SIDE):
    """The nearest even side that the contract allows, and `smallest` at the least."""
    value = max(smallest, min(MAX_SIDE, int(value)))
    return value - (value % 2)


class State:
    """What the program sends, and what the viewer told it."""

    def __init__(self, args):
        self.args = args
        self.width = even(args.width)
        self.height = even(args.height)
        # The size of the source is the size that this stand-in captures, and it
        # never sends a picture above it: a viewer asks for the size that it draws
        # at, and the program decides what it can send.
        self.source_width = self.width
        self.source_height = self.height
        self.pending = None
        self.old_size_left = 0
        self.sequence = 0
        self.frames_sent = 0
        self.frames_dropped = 0
        self.bytes = 0
        # The time of the capture of the last picture this program sent, in
        # microseconds since 1970. The `stats` message reports its age, so a viewer
        # that receives no picture (a hidden one) reads a growing number.
        self.last_captured_micros = None
        self.visible = True
        self.last_stats = time.monotonic()
        self.lock = threading.Lock()
        self.ended = threading.Event()
        self.socket = socket.socket(fileno=SOCKET_FD)


def picture(state):
    """One NV12 picture of the pattern of this program, with its sequence number."""
    with state.lock:
        width, height = state.width, state.height
        sequence = state.sequence
        state.sequence += 1
    background = 40
    bar = (sequence * 4) % max(width, 1)
    y = bytearray([background]) * (width * height)
    for row in range(height):
        start = row * width
        y[start + bar] = 200
        y[start + min(bar + 1, width - 1)] = 200
    origin_x, origin_y = 4, 4
    for row in range(min(7, height)):
        base = (origin_y + row) * width
        for column in range(min(13, width)):
            y[base + origin_x + column] = 16
    for index, character in enumerate(str(sequence)):
        glyph = DIGITS.get(character, DIGITS['0'])
        for row, line in enumerate(glyph):
            for column, pixel in enumerate(line):
                x = origin_x + 1 + index * 4 + column
                yy = origin_y + 1 + row
                if pixel == '1' and x < width and yy < height:
                    y[yy * width + x] = 235
    uv = bytearray([128]) * (width * height // 2)
    captured_micros = time.time_ns() // 1000
    with state.lock:
        state.last_captured_micros = captured_micros
    header = MAGIC + struct.pack('<QQII', sequence, captured_micros, width, height)
    return header, bytes(y), bytes(uv)


def garbage_picture(state):
    """One header of another magic, which no viewer of the contract reads."""
    with state.lock:
        width, height = state.width, state.height
    header = b'NV12BAD!' + struct.pack('<QQII', 0, 0, width, height)
    return header, bytes(width * height), bytes(width * height // 2)


def send_message(stream, message):
    """One control message: four bytes of length, most significant byte first."""
    body = json.dumps(message, separators=(',', ':')).encode('utf-8')
    stream.write(struct.pack('>I', len(body)))
    stream.write(body)
    stream.flush()


def send_picture(state):
    """Write one picture to the socket."""
    if state.args.garbage_header and state.frames_sent == 0:
        header, y, uv = garbage_picture(state)
    else:
        header, y, uv = picture(state)
    state.socket.sendall(header + y + uv)
    state.frames_sent += 1
    state.bytes += len(header) + len(y) + len(uv)


def send_stats(state, out):
    """Write one `stats` message when a whole second passed."""
    now = time.monotonic()
    if now - state.last_stats < 1.0:
        return
    state.last_stats = now
    with state.lock:
        width, height = state.width, state.height
    # The age of the newest picture, from its own capture to now: a viewer that
    # receives no picture reads a number that grows.
    with state.lock:
        last_captured_micros = state.last_captured_micros
    age_ms = 0
    if last_captured_micros is not None:
        age_ms = max(0, (time.time_ns() // 1000 - last_captured_micros) // 1000)
    send_message(out, {
        'type': 'stats',
        'frames_sent': state.frames_sent,
        'frames_dropped': state.frames_dropped,
        'bytes': state.bytes,
        'width': width,
        'height': height,
        'age_ms': age_ms,
    })


def handle_view(state, out, message):
    """Apply one `view`: a new size waits for the pictures of the old size."""
    width = message.get('width')
    height = message.get('height')
    visible = message.get('visible')
    with state.lock:
        if visible is not None:
            state.visible = bool(visible)
            phase = 'live' if state.visible else 'hidden'
        else:
            phase = None
        if width and height:
            wanted = (
                min(even(width), state.source_width),
                min(even(height), state.source_height),
            )
            if wanted != (state.width, state.height):
                state.pending = wanted
                state.old_size_left = state.args.old_size_pictures
    if phase is not None:
        send_message(out, {'type': 'state', 'phase': phase})


def switch_size(state, out):
    """Switch to the size that the viewer asked for, after the old-size pictures."""
    with state.lock:
        if state.old_size_left > 0:
            state.old_size_left -= 1
            return
        if state.pending is None:
            return
        state.width, state.height = state.pending
        state.pending = None
        width, height = state.width, state.height
        source_width, source_height = state.source_width, state.source_height
    send_message(out, {
        'type': 'stream',
        'width': width,
        'height': height,
        'source_width': source_width,
        'source_height': source_height,
    })


def read_exactly(descriptor, count):
    """`count` bytes of one descriptor, or fewer at its end.

    This reads the descriptor itself and not a buffered reader of it: a reader that a
    thread of this program holds may not hold a lock at the end of the interpreter.
    """
    chunks = []
    read = 0
    while read < count:
        chunk = os.read(descriptor, count - read)
        if not chunk:
            break
        chunks.append(chunk)
        read += len(chunk)
    return b''.join(chunks)


def read_control(state, out):
    """Read the control messages of the viewer until its standard input ends."""
    log = None
    if state.args.input:
        log = open(state.args.input, 'a', encoding='utf-8')
    try:
        while not state.ended.is_set():
            length_bytes = read_exactly(0, 4)
            if len(length_bytes) < 4:
                state.ended.set()
                return
            length = struct.unpack('>I', length_bytes)[0]
            if length > MAX_MESSAGE_BYTES:
                state.ended.set()
                return
            body = read_exactly(0, length)
            if len(body) < length:
                state.ended.set()
                return
            try:
                message = json.loads(body.decode('utf-8'))
            except (UnicodeDecodeError, json.JSONDecodeError):
                continue
            kind = message.get('type')
            if kind == 'bye':
                state.ended.set()
                return
            if kind == 'view':
                handle_view(state, out, message)
            elif kind == 'input' and log is not None:
                log.write(json.dumps(message, sort_keys=True) + '\n')
                log.flush()
    finally:
        if log is not None:
            log.close()


def write_report(state):
    """Write the process id and the state of descriptor 3, so a test proves both."""
    if not state.args.report:
        return
    try:
        descriptor = os.fstat(SOCKET_FD)
        is_socket = (descriptor.st_mode & 0o170000) == 0o140000
    except OSError:
        is_socket = False
    record = {
        'pid': os.getpid(),
        'socket_fd': SOCKET_FD,
        'socket': is_socket,
        'width': state.width,
        'height': state.height,
        'input': bool(state.args.input),
    }
    directory = os.path.dirname(os.path.abspath(state.args.report))
    os.makedirs(directory, exist_ok=True)
    with open(state.args.report, 'w', encoding='utf-8') as file:
        file.write(json.dumps(record) + '\n')


def parse_arguments(argv=None):
    """The options of one run."""
    parser = argparse.ArgumentParser(description='A stand-in frame source program for the tests of Horizon')
    parser.add_argument('--width', type=int, default=320, help='the width of the pictures, 16 to 8192, even')
    parser.add_argument('--height', type=int, default=180, help='the height of the pictures, 16 to 8192, even')
    parser.add_argument('--rate', type=float, default=10.0, help='the pictures of each second')
    parser.add_argument('--input', help='answer input messages and write each of them to this file as one JSON line')
    parser.add_argument('--fail-after', type=int, default=0, help='exit 1 with one line on the standard error')
    parser.add_argument('--busy', action='store_true', help='answer the error code busy and exit 0')
    parser.add_argument(
        '--old-size-pictures',
        type=int,
        default=0,
        help='pictures of the old size to send after a view that changes the size',
    )
    parser.add_argument('--garbage-header', action='store_true', help='send one header of another magic')
    parser.add_argument('--report', help='write the process id and the state of descriptor 3 to this file')
    return parser.parse_args(argv)


def main(argv=None):
    """Run one stand-in until it ends."""
    args = parse_arguments(argv)
    state = State(args)
    out = sys.stdout.buffer

    def terminate(_signal, _frame):
        state.ended.set()
        try:
            state.socket.close()
        except OSError:
            pass
        os._exit(0)

    signal.signal(signal.SIGTERM, terminate)
    signal.signal(signal.SIGINT, terminate)

    write_report(state)
    if args.busy:
        send_message(out, {'type': 'error', 'code': 'busy', 'message': 'another viewer holds this source'})
        return 0
    hello = {
        'type': 'hello',
        'protocol': 1,
        'formats': ['nv12'],
        'source': 'frame_source_standin',
        'session': 'standin-{}'.format(os.getpid()),
        'width': state.width,
        'height': state.height,
        'input': bool(args.input),
    }
    if not args.input:
        hello['input_reason'] = 'this stand-in reads no input'
    send_message(out, hello)
    send_message(out, {
        'type': 'stream',
        'width': state.width,
        'height': state.height,
        'source_width': state.source_width,
        'source_height': state.source_height,
    })

    reader = threading.Thread(target=read_control, args=(state, out), daemon=True)
    reader.start()
    try:
        while not state.ended.is_set():
            switch_size(state, out)
            if state.args.fail_after and state.frames_sent >= state.args.fail_after:
                print('the source of the stand-in failed after {} pictures'.format(state.frames_sent), file=sys.stderr)
                return 1
            if state.visible:
                try:
                    send_picture(state)
                except OSError:
                    raise Stop from None
            send_stats(state, out)
            time.sleep(0.01 if state.args.rate <= 0 else 1.0 / state.args.rate)
    except Stop:
        pass
    finally:
        state.ended.set()
        try:
            state.socket.close()
        except OSError:
            pass
    return 0


if __name__ == '__main__':
    sys.exit(main())
