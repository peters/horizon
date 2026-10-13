#!/usr/bin/env python3
"""Time browser tools in an agent session of a cloud worker, one JSON line every two seconds.

The probe opens its own browser panel through `horizon-browser mcp --connect` and loads a
page that runs 80 ms of JavaScript before DOMContentLoaded, so CPU starvation shows in the
navigation time. Stop it with SIGTERM or Ctrl-C; it closes its panel.
"""
import http.server
import json
import os
import signal
import subprocess
import sys
import threading
import time

PAGE = (b'<!doctype html><title>probe</title><canvas id="c" width="800" height="600"></canvas>'
        b'<script>let x = 0; const t = performance.now();'
        b'while (performance.now() - t < 80) { x += Math.sqrt(x + 1); }'
        b'const c = document.getElementById("c").getContext("2d");'
        b'for (let i = 0; i < 2000; i++) { c.fillStyle = "hsl(" + (i % 360) + ",80%,50%)";'
        b' c.fillRect(i % 800, (i * 7) % 600, 20, 20); }'
        b'document.title = "probe " + x.toFixed(0);</script>')


class Page(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header('Content-Type', 'text/html')
        self.send_header('Content-Length', str(len(PAGE)))
        self.end_headers()
        self.wfile.write(PAGE)

    def log_message(self, *_args):
        pass


class Mcp:
    def __init__(self):
        self.process = subprocess.Popen(['horizon-browser', 'mcp', '--connect'], stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, text=True)
        self.next_id = 0
        self.request('initialize', {'protocolVersion': '2025-06-18', 'capabilities': {},
                                    'clientInfo': {'name': 'browser-probe', 'version': '1'}})
        self.send({'jsonrpc': '2.0', 'method': 'notifications/initialized'})

    def send(self, message):
        self.process.stdin.write(json.dumps(message) + '\n')
        self.process.stdin.flush()

    def request(self, method, params):
        self.next_id += 1
        self.send({'jsonrpc': '2.0', 'id': self.next_id, 'method': method, 'params': params})
        for line in self.process.stdout:
            message = json.loads(line)
            if message.get('id') == self.next_id:
                if 'error' in message:
                    raise RuntimeError(message['error'])
                return message['result']
        raise RuntimeError('the MCP server stopped')

    def tool(self, name, arguments):
        result = self.request('tools/call', {'name': name, 'arguments': arguments})
        if result.get('isError'):
            raise RuntimeError(f'{name}: {result["content"][0]["text"]}')
        return result.get('structuredContent') or json.loads(result['content'][0]['text'])


def main():
    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Page)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f'http://127.0.0.1:{server.server_address[1]}/'
    mcp = Mcp()
    panel = mcp.tool('browser_create', {'url': base})['panel']['panel_id']
    stopping = []
    signal.signal(signal.SIGTERM, lambda *_: stopping.append(True))
    try:
        sample = 0
        while not stopping:
            sample += 1
            started = time.monotonic()
            mcp.tool('browser_navigate', {'panel_id': panel, 'url': f'{base}?{sample}',
                                          'wait': 'domcontentloaded'})
            navigated = time.monotonic()
            mcp.tool('browser_snapshot', {'panel_id': panel, 'max_nodes': 50})
            done = time.monotonic()
            print(json.dumps({'time': round(time.time(), 1), 'sample': sample,
                              'navigate_ms': round((navigated - started) * 1000),
                              'snapshot_ms': round((done - navigated) * 1000),
                              'load1': os.getloadavg()[0]}), flush=True)
            time.sleep(2)
    except KeyboardInterrupt:
        pass
    finally:
        mcp.tool('browser_close', {'panel_id': panel})
        mcp.process.stdin.close()
        mcp.process.wait(10)


if __name__ == '__main__':
    sys.exit(main())
