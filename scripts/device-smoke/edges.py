#!/usr/bin/env python3
"""Assert MCP cancellation/disconnect releases in-flight input on an owned X11 lab."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import time
from client import Client

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary', type=Path, required=True)
parser.add_argument('--target', type=Path, required=True)
args = parser.parse_args()
display = json.loads(args.target.read_text())['endpoint']['display']
env = dict(os.environ, DISPLAY=display)


def pointer():
    return subprocess.check_output(['xinput', '--query-state', 'Virtual core XTEST pointer'],
                                   env=env, text=True, timeout=5)


def wait_button(down):
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if ('button[1]=down' in pointer()) == down:
            return
        time.sleep(0.02)
    raise AssertionError(f'button did not become {"pressed" if down else "released"}')


def start_drag(client):
    geometry = client.call('screenshot')['geometry']
    client.sequence += 1
    request_id = client.sequence
    request = {'jsonrpc': '2.0', 'id': request_id, 'method': 'tools/call',
               'params': {'name': 'device_act', 'arguments': {'geometry': geometry,
                          'action': {'kind': 'drag', 'from': {'x': 1100, 'y': 180},
                                     'to': {'x': 1150, 'y': 220}, 'duration_ms': 2000}}}}
    client.process.stdin.write(json.dumps(request) + '\n')
    client.process.stdin.flush()
    wait_button(True)
    return request_id


client = Client(args.binary, args.target, 'mcp')
try:
    request_id = start_drag(client)
    client.process.stdin.write(json.dumps({'jsonrpc': '2.0', 'method': 'notifications/cancelled',
                                          'params': {'requestId': request_id, 'reason': 'smoke'}}) + '\n')
    client.process.stdin.flush()
    wait_button(False)
    assert client.process.poll() is None, 'cancellation closed the MCP transport'
    assert '=down' not in pointer(), 'cancellation left an input button held'
    # A second drag isolates disconnect cleanup from cancellation cleanup.
    start_drag(client)
finally:
    client.close()
assert '=down' not in pointer(), 'disconnect left an input button held'
print('PASS: cancellation releases input while connected; a separate disconnect releases input')
