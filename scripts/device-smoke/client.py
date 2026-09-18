#!/usr/bin/env python3
"""Small CLI/MCP smoke client; it never controls a browser."""
import argparse
import base64
import json
import os
from pathlib import Path
import select
import subprocess
import time


class Client:
    def __init__(self, binary, target, mode):
        self.args = [str(binary), '--target', str(target)]
        self.process = None
        self.sequence = 0
        self.pending = b''
        if mode == 'mcp':
            self.process = subprocess.Popen(self.args + ['mcp'], stdin=subprocess.PIPE,
                                            stdout=subprocess.PIPE, text=True, bufsize=1)
            self.rpc('initialize', {'protocolVersion': '2025-11-25', 'capabilities': {},
                                    'clientInfo': {'name': 'device-smoke', 'version': '1'}})
            self.process.stdin.write(json.dumps({'jsonrpc': '2.0', 'method': 'notifications/initialized'}) + '\n')
            self.process.stdin.flush()
            tools = self.rpc('tools/list', {})
            assert {'device_doctor', 'device_screenshot', 'device_act', 'device_resize'} <= {t['name'] for t in tools['tools']}

    def rpc(self, method, params):
        self.sequence += 1
        self.process.stdin.write(json.dumps({'jsonrpc': '2.0', 'id': self.sequence,
                                            'method': method, 'params': params}) + '\n')
        self.process.stdin.flush()
        deadline = time.monotonic() + 20
        while True:
            while b'\n' not in self.pending:
                remaining = max(0, deadline - time.monotonic())
                if remaining <= 0 or not select.select([self.process.stdout], [], [], remaining)[0]:
                    raise TimeoutError(method)
                chunk = os.read(self.process.stdout.fileno(), 65536)
                if not chunk:
                    raise RuntimeError('MCP process exited')
                self.pending += chunk
            line, self.pending = self.pending.split(b'\n', 1)
            value = json.loads(line)
            if value.get('id') == self.sequence:
                if 'error' in value:
                    raise RuntimeError(value['error'])
                return value['result']

    def call(self, name, request=None):
        if self.process:
            response = self.rpc('tools/call', {'name': 'device_' + name, 'arguments': request or {}})
            texts = [c['text'] for c in response['content'] if c['type'] == 'text']
            value = json.loads(texts[0])
            for content in response['content']:
                if content['type'] == 'image':
                    value['result']['image_base64'] = content['data']
        else:
            command = self.args + [name]
            if request is not None:
                command += [json.dumps(request)]
            result = subprocess.run(command, text=True, capture_output=True, timeout=20)
            value = json.loads(result.stdout)
            assert (result.returncode == 0) == value['ok'], result
        if not value['ok']:
            raise RuntimeError(value)
        return value['result']

    def close(self):
        if self.process:
            self.process.stdin.close()
            self.process.wait(timeout=10)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--target', type=Path, required=True)
    parser.add_argument('--mode', choices=['cli', 'mcp'], required=True)
    parser.add_argument('--output', type=Path, required=True)
    group = parser.add_mutually_exclusive_group()
    group.add_argument('--actions', default='[]', help='JSON list of Action objects')
    group.add_argument('--actions-file', type=Path, help='File containing a JSON Action list')
    args = parser.parse_args()
    args.output.mkdir(mode=0o700, parents=True, exist_ok=False)
    client = Client(args.binary, args.target, args.mode)
    report = []
    try:
        print(json.dumps(client.call('doctor')), flush=True)
        for i, action in enumerate([None] + json.loads(args.actions_file.read_text() if args.actions_file else args.actions)):
            if action:
                before = client.call('screenshot')
                report.append(client.call('act', {'geometry': before['geometry'], 'action': action}))
                time.sleep(1.3)
            observation = client.call('screenshot')
            image = observation.pop('image_base64')
            (args.output / f'{i:02}.png').write_bytes(base64.b64decode(image))
            report.append(observation)
        (args.output / 'report.json').write_text(json.dumps(report, indent=2))
    finally:
        client.close()


if __name__ == '__main__':
    main()
