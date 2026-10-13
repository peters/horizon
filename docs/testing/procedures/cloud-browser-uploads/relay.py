#!/usr/bin/env python3
"""Drive the upload relay of a cloud worker control service with a synthetic page.

The driver speaks the cloud view protocol as a host does. By default it connects to
a control service on this computer (127.0.0.1:47280). With --ssh it runs
`horizon-cloud-worker connect` on a worker through the given SSH command, and it
serves the test page on the worker loopback. It prints one line for each check and
`RELAY PASS` at the end.

Usage: relay.py [--ssh "ssh -i KEY root@HOST"] [--engine chromium|firefox]
"""
import argparse
import base64
import hashlib
import http.server
import json
import os
import shlex
import socket
import subprocess
import sys
import threading
import time
import urllib.parse

PAGE = b"""<!doctype html><meta charset=utf-8><title>Upload page</title>
<style>body{margin:0}#pick{position:fixed;left:0;top:0;width:400px;height:150px}
#zone{position:fixed;left:0;top:200px;width:400px;height:200px;background:#ddd}</style>
<input id=pick type=file multiple><div id=zone>Drop files</div><script>
let reports = 0;
const report = text => { location.hash = ++reports + ':' + encodeURIComponent(text); };
const hex = buffer => [...new Uint8Array(buffer)].map(b => b.toString(16).padStart(2, '0')).join('');
async function describe(files) {
  const parts = [];
  for (const file of files) parts.push(`${file.name}|${file.size}|${hex(await crypto.subtle.digest('SHA-256', await file.arrayBuffer()))}`);
  return parts.sort().join(';');
}
pick.addEventListener('change', async () => report('picked:' + await describe(pick.files)));
pick.addEventListener('cancel', () => report('cancelled'));
zone.addEventListener('dragover', event => event.preventDefault());
zone.addEventListener('drop', async event => { event.preventDefault(); report('dropped:' + await describe(event.dataTransfer.files)); });
</script>"""
PAGE_PORT = 47290
CHUNK = 4 << 20


class Connection:
    """One JSON-lines connection to the control service."""

    def __init__(self, ssh):
        if ssh:
            self.process = subprocess.Popen([*ssh, 'horizon-cloud-worker connect'], stdin=subprocess.PIPE,
                                            stdout=subprocess.PIPE, bufsize=0)
            self.write, self.stream = self.process.stdin.write, self.process.stdout
        else:
            self.process = None
            sock = socket.create_connection(('127.0.0.1', 47280), timeout=40)
            self.write, self.stream = sock.sendall, sock.makefile('rb')

    def call(self, message):
        self.write((json.dumps(message) + '\n').encode())
        line = self.stream.readline(1 << 27)
        if not line:
            sys.exit('the control service closed the connection')
        return json.loads(line)

    def close(self):
        if self.process:
            self.process.stdin.close()
            self.process.wait(30)


def serve_page(ssh):
    if ssh:
        script = (f'mkdir -p /tmp/relay-page && cat > /tmp/relay-page/index.html && cd /tmp/relay-page && '
                  f'(setsid python3 -m http.server {PAGE_PORT} --bind 127.0.0.1 >/dev/null 2>&1 < /dev/null &)')
        subprocess.run([*ssh, script], input=PAGE, check=True, timeout=60)
        time.sleep(1)
        return f'http://127.0.0.1:{PAGE_PORT}/'

    class Page(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            self.send_response(200)
            self.send_header('Content-Type', 'text/html')
            self.send_header('Content-Length', str(len(PAGE)))
            self.end_headers()
            self.wfile.write(PAGE)

        def log_message(self, *_args):
            pass
    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Page)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return f'http://127.0.0.1:{server.server_address[1]}/'


def click(x, y):
    press = {'type': 'mouse_press', 'x': x, 'y': y, 'button': 'left', 'click_count': 1, 'buttons': 1, 'modifiers': {}}
    return [{'Input': press}, {'Input': dict(press, type='mouse_release', buttons=0)}]


def page_report(state):
    count, _, text = state['url'].partition('#')[2].partition(':')
    return (int(count), urllib.parse.unquote(text)) if count.isdigit() else (0, '')


def described(files):
    return ';'.join(sorted(f'{name}|{len(data)}|{hashlib.sha256(data).hexdigest()}' for name, data in files.items()))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--ssh', type=shlex.split, default=None)
    parser.add_argument('--engine', default='chromium', choices=['chromium', 'firefox'])
    args = parser.parse_args()
    browser = f'relay-{os.getpid()}'
    view, transfer = Connection(args.ssh), Connection(args.ssh)

    def poll(uploads=None, commands=(), until=lambda state: True, label='', timeout=60):
        deadline = time.monotonic() + timeout
        while True:
            message = {'operation': 'poll', 'id': browser, 'after': 0, 'commands': list(commands)}
            if uploads is not None:
                message['uploads'] = uploads
            reply = view.call(message)
            if reply.get('error'):
                sys.exit(f'{label}: {reply["error"]}')
            state = reply['browsers'][0]
            if until(state):
                return state
            if time.monotonic() > deadline:
                state.pop('png', None)
                sys.exit(f'{label}: timed out: {json.dumps(state)[:2000]}')
            commands, uploads = (), uploads and {'version': 1}
            time.sleep(.15)

    def stage(batch, name, data):
        parts = [data[offset:offset + CHUNK] for offset in range(0, len(data), CHUNK)] or [b'']
        for index, part in enumerate(parts):
            last = index == len(parts) - 1
            reply = transfer.call({'operation': 'stage', 'id': browser, 'batch': batch, 'name': name,
                                   'size': len(data), 'offset': index * CHUNK,
                                   'data': base64.b64encode(part).decode(),
                                   **({'sha256': hashlib.sha256(data).hexdigest()} if last else {})})
            if reply.get('error'):
                sys.exit(f'stage {name}: {reply["error"]}')
        return reply['staged']

    url = serve_page(args.ssh)
    opened = view.call({'operation': 'open', 'id': browser, 'url': url, 'backend': args.engine})
    if opened.get('error'):
        sys.exit(f'open: {opened["error"]}')
    state = poll(until=lambda state: state['ready'] and state['url'].startswith(url), label='ready', timeout=120)
    uploads = state.get('uploads')
    if uploads is None:
        sys.exit('the worker has no upload support: an older worker, or HORIZON_CLOUD_BROWSER_UPLOADS is not 1')
    print('ready: file_chooser', uploads['file_chooser'], 'drops', uploads['drops'], 'limits', uploads['limits'])
    try:
        if uploads['file_chooser']:
            poll(None, click(100, 75), lambda state: page_report(state)[1] == 'cancelled', 'older host')
            print('older host: the worker cancelled the chooser at once')
            poll({'version': 1}, click(100, 75), lambda state: state['uploads'].get('request'), 'chooser')
            request = poll({'version': 1})['uploads']['request']
            print('chooser request:', request)
            files = {"Rapport ñ it's 1.txt": b'synthetic text\n' * 1000, 'photo.png': os.urandom(CHUNK + 4099)}
            for name, data in files.items():
                print('staged', name, stage('b1', name, data))
            answer = {'request': request['id'], 'files': {'batch': 'b1', 'names': list(files)}}
            state = poll({'version': 1, 'answer': answer},
                         until=lambda state: page_report(state)[1].startswith('picked:'), label='answer')
            if page_report(state)[1] != 'picked:' + described(files):
                sys.exit(f'the page received other files: {page_report(state)[1]}')
            print('page received: names, sizes and SHA-256 agree')
        if uploads['drops']:
            dropped = {'drop me.txt': b'dropped bytes'}
            page = poll({'version': 1})['uploads']['page']
            stage('b2', 'drop me.txt', dropped['drop me.txt'])
            drop = {'page': page, 'x': 100, 'y': 300, 'files': {'batch': 'b2', 'names': ['drop me.txt']}}
            state = poll({'version': 1, 'drop': drop},
                         until=lambda state: page_report(state)[1].startswith('dropped:'), label='drop')
            if page_report(state)[1] != 'dropped:' + described(dropped):
                sys.exit(f'the page received another drop: {page_report(state)[1]}')
            print('drop received: name, size and SHA-256 agree')
        if uploads['file_chooser']:
            poll({'version': 1}, click(100, 75), lambda state: state['uploads'].get('request'), 'second chooser')
            started = time.monotonic()
            poll(None, until=lambda state: page_report(state)[1] == 'cancelled', label='host left', timeout=30)
            print(f'host left: the worker cancelled the chooser after {time.monotonic() - started:.0f} s')
    finally:
        closed = view.call({'operation': 'close', 'id': browser})
        print('closed:', closed.get('closed'), closed.get('error') or '')
        view.close()
        transfer.close()
    print('RELAY PASS')


if __name__ == '__main__':
    main()
