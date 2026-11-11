#!/usr/bin/env python3
"""A stand-in transcription service for a speech test, on one loopback address.

The service answers one `multipart/form-data` request with a JSON object that holds
the text of `--text`, as a transcription service does. It records what it received
in `--record`, and it records no credential: the `Authorization` header and its
value are left out of that record, because a test that reads it must prove that no
other file of the run holds the key.

The service is a stand-in of this repository, so a test of speech input sends no
recording to a real service. Run it on the loopback address of this computer and
name its URL in the settings:

    /usr/bin/python3 -B scripts/device-smoke/speech_standin_service.py \\
        --record /tmp/received.json --text hei

It prints the address that it listens on, such as `127.0.0.1:40913`, and it ends
after `--requests` requests, so no process stays behind.
"""
from __future__ import annotations

import argparse
import hashlib
import http.server
import json
from pathlib import Path
import sys
import threading

# The path that the settings of a run name, as the endpoint of the service.
TRANSCRIPTION_PATH = '/v1/audio/transcriptions'

# The most bytes of one request that the stand-in reads.
MAX_REQUEST_BYTES = 64 * 1024 * 1024

# The headers that a record keeps: none of them carries a credential.
KEPT_HEADERS = ('host', 'content-type', 'content-length', 'accept', 'user-agent')


def field_names(body, boundary):
    """The names of the form fields of one multipart body, in the order of the body."""
    names = []
    marker = b'--' + boundary
    for part in body.split(marker):
        head = part.split(b'\r\n\r\n', 1)[0]
        for line in head.split(b'\r\n'):
            if line.lower().startswith(b'content-disposition:') and b'name="' in line:
                name = line.split(b'name="', 1)[1].split(b'"', 1)[0]
                names.append(name.decode('utf-8', 'replace'))
    return names


def boundary_of(content_type):
    """The boundary of one `multipart/form-data` content type, or `None`."""
    for part in content_type.split(';'):
        part = part.strip()
        if part.startswith('boundary='):
            return part[len('boundary='):].strip('"').encode()
    return None


def describe(method, path, headers, body):
    """What one request held, with no credential of it.

    The `Authorization` header and its value never reach this record. The body is
    hashed and its form fields are named, so a test proves the shape of the request
    without keeping a recording in a file.
    """
    kept = {name: value for name, value in headers.items() if name.lower() in KEPT_HEADERS}
    content_type = headers.get('Content-Type', '')
    boundary = boundary_of(content_type)
    return {
        'method': method,
        'path': path,
        'headers': kept,
        'authorization': 'redacted' if headers.get('Authorization') else 'none',
        'bytes': len(body),
        'sha256': hashlib.sha256(body).hexdigest(),
        'fields': field_names(body, boundary) if boundary else [],
    }


class Handler(http.server.BaseHTTPRequestHandler):
    """One transcription request: record it, and answer the text."""

    # The text that the service answers, the status, the message of a refusal,
    # and the record that it writes.
    text = 'hei'
    status = 200
    error = ''
    record = None

    def do_POST(self):  # noqa: N802 - the name of the base class
        length = int(self.headers.get('Content-Length', '0'))
        body = self.rfile.read(min(length, MAX_REQUEST_BYTES))
        request = describe(self.command, self.path, self.headers, body)
        if self.record is not None:
            self.record.write_text(json.dumps(request, indent=2) + '\n')
        if self.path != TRANSCRIPTION_PATH:
            self.send_error(404, 'no transcription endpoint here')
            return
        if self.status != 200:
            answer = json.dumps({'error': {'message': self.error or 'refused'}}).encode()
        else:
            answer = json.dumps({'text': self.text}).encode()
        self.send_response(self.status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(answer)))
        self.end_headers()
        self.wfile.write(answer)

    def log_message(self, _format, *args):  # noqa: A003 - the name of the base class
        """Say nothing: the record of the request is the evidence of a run."""


def serve(address, text, record, requests=1, status=200, error=''):
    """Serve `requests` requests on `address`, and answer the address that it took.

    The server runs on a thread, so a caller of a test keeps its own frame. An
    address with port 0 takes a free port of this computer, and the answer names it.
    """
    handler = type('RecordedHandler', (Handler,), {
        'text': text, 'record': record, 'status': status, 'error': error})
    server = http.server.HTTPServer(address, handler)
    served = []

    def run():
        for _ in range(max(1, requests)):
            server.handle_request()
        server.server_close()

    thread = threading.Thread(target=run, daemon=True)
    thread.start()
    served.append(thread)
    return server.server_address, thread


def parse_arguments(argv):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bind', default='127.0.0.1', help='The address of this computer to listen on')
    parser.add_argument('--port', type=int, default=0, help='The port to listen on, or 0 for a free one')
    parser.add_argument('--text', default='hei', help='The text that the service answers')
    parser.add_argument('--status', type=int, default=200,
                        help='The status of the answer, such as 401 for a refused key')
    parser.add_argument('--error', default='', help='The message of a refused answer')
    parser.add_argument('--record', type=Path, help='The file that holds what one request carried')
    parser.add_argument('--requests', type=int, default=1, help='The number of requests before the end')
    return parser.parse_args(argv)


def main(argv):
    args = parse_arguments(argv)
    address, thread = serve((args.bind, args.port), args.text, args.record, args.requests,
                            args.status, args.error)
    print(f'{address[0]}:{address[1]}', flush=True)
    try:
        thread.join()
    except KeyboardInterrupt:
        pass
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
