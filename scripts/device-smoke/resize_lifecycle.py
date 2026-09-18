#!/usr/bin/env python3
"""Resize cancellation contracts against an explicit disposable X11 target."""
import argparse
import json
import os
import select
import socket
import struct
import subprocess
import tempfile
import threading
import time
from pathlib import Path
from client import Client


def exact(stream, count):
    value = b''
    while len(value) < count:
        part = stream.recv(count-len(value))
        if not part:
            raise EOFError
        value += part
    return value


class ResizeServer:
    """Advertise a matching desktop, accept resize, and withhold confirmation."""
    def __init__(self, geometry):
        self.width, self.height = geometry['width'], geometry['height']
        self.listener = socket.socket()
        self.listener.bind(('127.0.0.1', 0))
        self.listener.listen()
        self.address = '127.0.0.1:' + str(self.listener.getsockname()[1])
        self.received = threading.Event()
        self.requests = 0
        self.stopped = threading.Event()
        self.clients = []
        self.threads = []
        self.acceptor = threading.Thread(target=self.accept, daemon=True)
        self.acceptor.start()

    def accept(self):
        while not self.stopped.is_set():
            try:
                stream, _ = self.listener.accept()
            except OSError:
                break
            self.clients.append(stream)
            thread = threading.Thread(target=self.serve, args=(stream,), daemon=True)
            self.threads.append(thread)
            thread.start()

    def serve(self, stream):
        try:
            stream.sendall(b'RFB 003.008\n')
            exact(stream, 12)
            stream.sendall(b'\x01\x01')
            exact(stream, 1)
            stream.sendall(bytes(4))
            exact(stream, 1)
            w, h = self.width, self.height
            stream.sendall(struct.pack('>HH', w, h) + bytes([32,24,0,1]) +
                           struct.pack('>HHHBBBxxxI', 255,255,255,0,8,16,4) + b'test')
            announced = False
            while not self.stopped.is_set():
                kind = exact(stream, 1)[0]
                if kind == 0:
                    exact(stream, 19)
                elif kind == 2:
                    header = exact(stream, 3)
                    exact(stream, struct.unpack('>H', header[1:])[0]*4)
                elif kind == 3:
                    exact(stream, 9)
                    if not announced:
                        stream.sendall(bytes([0,0,0,1]) + struct.pack('>HHHHi', 0,0,w,h,-308) +
                                       bytes([1,0,0,0]) + struct.pack('>IHHHHI', 7,0,0,w,h,0))
                        announced = True
                elif kind == 251:
                    header = exact(stream, 7)
                    exact(stream, header[5]*16)
                    self.requests += 1
                    self.received.set()
                else:
                    raise AssertionError('unexpected wire request: ' + str(kind))
        except (EOFError, ConnectionError, OSError):
            pass
        finally:
            stream.close()

    def close(self):
        self.stopped.set()
        self.listener.shutdown(socket.SHUT_RDWR)
        self.listener.close()
        for stream in self.clients:
            try:
                stream.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
        self.acceptor.join(timeout=2)
        for thread in self.threads:
            thread.join(timeout=2)
        assert not self.acceptor.is_alive()
        assert all(not thread.is_alive() for thread in self.threads)


def invoke(binary, target, command, request=None):
    args = [str(binary), '--target', str(target), command]
    if request is not None:
        args.append(json.dumps(request))
    result = subprocess.run(args, capture_output=True, text=True, timeout=20)
    return json.loads(result.stdout)


def response_before(client, request_id, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        while b'\n' not in client.pending:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not select.select([client.process.stdout], [], [], remaining)[0]:
                raise TimeoutError('resize response')
            chunk = os.read(client.process.stdout.fileno(), 65536)
            if not chunk:
                raise RuntimeError('MCP process exited')
            client.pending += chunk
        line, client.pending = client.pending.split(b'\n', 1)
        response = json.loads(line)
        if response.get('id') == request_id:
            return response
    raise TimeoutError('resize response')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--target', type=Path, required=True)
    args = parser.parse_args()
    base = json.loads(args.target.read_text())
    geometry = invoke(args.binary, args.target, 'doctor')['result']['geometry']
    request = {'width': geometry['width']+1, 'height': geometry['height']}
    for mode in ['cli-terminated', 'mcp-cancelled', 'cli-timeout', 'mcp-timeout']:
        server = ResizeServer(geometry)
        client = None
        process = None
        try:
            with tempfile.TemporaryDirectory(prefix='resize-contract-') as directory:
                target = Path(directory)/'target.json'
                target.write_text(json.dumps(dict(base, desktop_resize={
                    'policy': {'enabled': True}, 'vnc_address': server.address})))
                if mode.startswith('cli-'):
                    process = subprocess.Popen([str(args.binary), '--target', str(target), 'resize', json.dumps(request)],
                                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                else:
                    client = Client(args.binary, target, 'mcp')
                    client.sequence += 1
                    request_id = client.sequence
                    client.process.stdin.write(json.dumps({'jsonrpc':'2.0','id':request_id,'method':'tools/call',
                        'params':{'name':'device_resize','arguments':request}})+'\n')
                    client.process.stdin.flush()
                assert server.received.wait(timeout=10), 'resize was never dispatched'
                busy = invoke(args.binary, target, 'doctor')
                assert not busy['ok'] and 'busy' in busy['error']['message'], busy
                if mode == 'cli-timeout':
                    response = json.loads(process.communicate(timeout=10)[0])
                    assert response['error']['code'] == 'resize_timeout', response
                    assert response['error']['resize_uncertain'] is True, response
                elif mode == 'mcp-timeout':
                    response = response_before(client, request_id)
                    error = json.loads(response['result']['content'][0]['text'])['error']
                    assert error['code'] == 'resize_timeout' and error['resize_uncertain'] is True, error
                elif process:
                    # Terminate only the child this scenario created, after wire dispatch.
                    process.kill()
                    process.communicate(timeout=10)
                else:
                    client.process.stdin.write(json.dumps({'jsonrpc':'2.0','method':'notifications/cancelled',
                        'params':{'requestId':request_id,'reason':'contract test'}})+'\n')
                    client.process.stdin.flush()
                    busy = invoke(args.binary, target, 'doctor')
                    assert not busy['ok'] and 'busy' in busy['error']['message'], busy
                    time.sleep(6)
                assert target.with_name(target.name + '.resize-pending').is_file()
                readiness = invoke(args.binary, target, 'doctor')['result']['desktop_resize']
                assert readiness['supported'] and readiness['uncertain'], readiness
                retry = invoke(args.binary, target, 'resize', request)
                assert not retry['ok'] and retry['error']['code']=='resize_uncertain', retry
                assert retry['error']['resize_uncertain'] is True, retry
                assert server.requests == 1, 'uncertain resize was dispatched again'
                print(mode, 'PASS: serialized, uncertainty persisted, no blind retry', flush=True)
                if client:
                    client.close()
                    client = None
        finally:
            if client:
                client.close()
            if process and process.poll() is None:
                process.kill()
                process.communicate(timeout=10)
            server.close()


if __name__ == '__main__':
    main()
