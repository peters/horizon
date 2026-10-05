"""Synthetic interoperability receiver. Never connects to a physical television."""
import socket, threading, plistlib, struct, time, json, ipaddress
from pathlib import Path
from auth import Reference, HAPSession, read_tlv, TlvValue, hkdf_expand
from cryptography.hazmat.primitives.ciphers.aead import ChaCha20Poly1305

class Receiver:

    def __init__(self, root, ip='127.0.0.1', label='Synthetic-Cast', split_streams=False):
        if not ipaddress.ip_address(ip).is_loopback:
            raise ValueError('benchmark receiver must bind loopback')
        self.split_streams = split_streams
        self.controllers = {}
        clock_id = 4242
        self.root = Path(root)
        self.clock_id = clock_id
        self.ip = ip
        self.label = label
        self.errors = []
        self.sessions = []
        self.closed = False
        self.lock = threading.Lock()
        self.listen = socket.socket()
        self.listen.bind((ip, 0))
        self.listen.listen()
        self.listen.settimeout(0.5)
        self.port = self.listen.getsockname()[1]
        self.threads = []
        self.sockets = {self.listen}
        self.thread = threading.Thread(target=self.run)
        self.thread.start()

    def run(self):
        while not self.closed:
            try:
                conn, _ = self.listen.accept()
            except socket.timeout:
                continue
            except OSError:
                break
            if self.track(conn):
                t = threading.Thread(target=self.control, args=(conn,))
                with self.lock:
                    t.start()
                    self.threads.append(t)

    def track(self, connection):
        with self.lock:
            if self.closed:
                connection.close()
                return False
            self.sockets.add(connection)
            return True

    def listener(self):
        listener = socket.socket()
        if not self.track(listener):
            raise RuntimeError('receiver closed during setup')
        listener.bind((self.ip, 0))
        listener.listen()
        listener.settimeout(12)
        return listener

    def control(self, conn):
        state = {'frames': 0, 'configs': 0, 'feedback': 0, 'teardown': False, 'events': False}
        with self.lock:
            self.sessions.append(state)
            state['stream_file'] = f'received-{len(self.sessions)}.h264' if self.split_streams else 'received.h264'
        events = video = None
        workers = []
        try:
            events = self.listener()
            video = self.listener()
            conn.settimeout(190)
            auth = Reference('Synthetic Receiver', unique_id='synthetic-' + self.label, pin=1234, controllers=self.controllers)
            auth.ready = None
            cipher = HAPSession()
            buffer = b''
            encrypted = False
            while True:
                while b'\r\n\r\n' not in buffer:
                    part = conn.recv(65536)
                    if not part:
                        return
                    buffer += cipher.decrypt(part)
                head, buffer = buffer.split(b'\r\n\r\n', 1)
                lines = head.decode().split('\r\n')
                method, path, _ = lines[0].split(' ')
                headers = dict((line.split(': ', 1) for line in lines[1:]))
                length = int(headers.get('Content-Length', '0'))
                while len(buffer) < length:
                    part = conn.recv(65536)
                    if not part:
                        raise EOFError()
                    buffer += cipher.decrypt(part)
                body, buffer = (buffer[:length], buffer[length:])
                reply = b''
                if path == '/pair-pin-start':
                    state['pin_requests'] = state.get('pin_requests', 0) + 1
                elif path in ['/pair-setup', '/pair-verify']:
                    state[path] = state.get(path, 0) + 1
                    fields = read_tlv(body)
                    step = fields[TlvValue.SeqNo][0]
                    reply = getattr(auth, f'_m{step}_{('setup' if path.endswith('setup') else 'verify')}')(fields, *([False] if path.endswith('setup') else []))
                elif path == '/info':
                    reply = plistlib.dumps({'model': 'AppleTV14,1', 'features': 1 << 41}, fmt=plistlib.FMT_BINARY)
                elif method == 'SETUP':
                    setup = plistlib.loads(body)
                    if 'streams' not in setup:
                        assert setup['timingProtocol'] == 'PTP' and setup['isScreenMirroringSession']
                        reply = plistlib.dumps({'timingPeerInfo': {'ClockID': self.clock_id}, 'eventPort': events.getsockname()[1]}, fmt=plistlib.FMT_BINARY)
                        worker = threading.Thread(target=self.events, args=(events, auth.shared_key, state))
                        worker.start()
                        workers.append(worker)
                    else:
                        stream = setup['streams'][0]
                        assert stream['type'] == 110
                        reply = plistlib.dumps({'streams': [{'type': 110, 'dataPort': video.getsockname()[1]}]}, fmt=plistlib.FMT_BINARY)
                        worker = threading.Thread(target=self.video, args=(video, auth.shared_key, stream['streamConnectionID'], state))
                        worker.start()
                        workers.append(worker)
                elif method == 'RECORD':
                    pass
                elif path == '/feedback':
                    state['feedback'] += 1
                elif method == 'TEARDOWN':
                    state['teardown'] = True
                else:
                    raise AssertionError((method, path))
                timestamp = int(time.monotonic() * 1000)
                response = f'HTTP/1.1 200 OK\r\nCSeq: {headers.get('CSeq', '0')}\r\nX-Apple-RequestReceivedTimestamp: {timestamp}\r\nX-Apple-ProcessingTime: 0\r\nContent-Length: {len(reply)}\r\n\r\n'.encode() + reply
                conn.sendall(cipher.encrypt(response))
                if auth.ready and (not encrypted):
                    cipher.enable(*auth.ready)
                    encrypted = True
                if method == 'TEARDOWN':
                    break
        except Exception as error:
            self.errors.append(repr(error))
        finally:
            for connection in [conn, events, video]:
                if connection is not None:
                    connection.close()
            for worker in workers:
                worker.join(15)
            (self.root / 'receiver.json').write_text(json.dumps({'sessions': self.sessions, 'errors': self.errors}, indent=2))

    def events(self, listener, secret, state):
        try:
            conn, _ = listener.accept()
            if not self.track(conn):
                return
            conn.settimeout(15)
            with conn:
                cipher = HAPSession()
                cipher.enable(hkdf_expand('Events-Salt', 'Events-Write-Encryption-Key', secret), hkdf_expand('Events-Salt', 'Events-Read-Encryption-Key', secret))
                conn.sendall(cipher.encrypt(b'POST /event RTSP/1.0\r\nCSeq: 1\r\nContent-Length: 0\r\n\r\n'))
                response = b''
                while b'\r\n\r\n' not in response:
                    data = conn.recv(4096)
                    if not data:
                        raise EOFError()
                    response += cipher.decrypt(data)
                assert response.startswith(b'RTSP/1.0 200')
                state['events'] = True
                conn.settimeout(None)
                while conn.recv(4096):
                    pass
        except Exception as error:
            self.errors.append('event:' + repr(error))

    def video(self, listener, secret, stream, state):

        def exact(conn, n, allow_eof=False):
            result = b''
            while len(result) < n:
                part = conn.recv(n - len(result))
                if not part:
                    if allow_eof and not result:
                        raise EOFError()
                    raise RuntimeError('truncated video packet')
                result += part
            return result
        try:
            conn, _ = listener.accept()
            if not self.track(conn):
                return
            conn.settimeout(8)
            counter = 0
            previous = 0
            cipher = ChaCha20Poly1305(hkdf_expand(f'DataStream-Salt{stream}', 'DataStream-Output-Encryption-Key', secret))
            pending_configuration = bytearray()
            with conn, (self.root / state['stream_file']).open('wb') as output:
                while True:
                    head = exact(conn, 128, allow_eof=True)
                    length = struct.unpack_from('<I', head)[0]
                    assert length < 9 * 1024 * 1024
                    body = exact(conn, length)
                    if head[4] == 1:
                        state['configs'] += 1
                        state['dimensions'] = list(struct.unpack_from('<ff', head, 16))
                        state.setdefault('config_dimensions', []).extend(
                            list(struct.unpack_from('<ff', head, at)) for at in [16, 40, 56])
                        assert body[0] == 1
                        configuration = bytearray()
                        offset = 6
                        for _ in range(body[5] & 31):
                            n = struct.unpack_from('>H', body, offset)[0]
                            offset += 2
                            assert n > 0 and offset + n <= len(body)
                            configuration.extend(b'\x00\x00\x00\x01' + body[offset:offset + n])
                            offset += n
                        count = body[offset]
                        offset += 1
                        for _ in range(count):
                            n = struct.unpack_from('>H', body, offset)[0]
                            offset += 2
                            assert n > 0 and offset + n <= len(body)
                            configuration.extend(b'\x00\x00\x00\x01' + body[offset:offset + n])
                            offset += n
                        assert offset == len(body)
                        if self.split_streams:
                            pending_configuration[:] = configuration
                            state['pending_configuration_bytes'] = len(configuration)
                        else:
                            output.write(configuration)
                    else:
                        assert head[4] == 0 and struct.unpack_from('<Q', head, 40)[0] == self.clock_id & (1 << 64) - 1
                        stamp = struct.unpack_from('<Q', head, 8)[0]
                        assert stamp > previous
                        previous = stamp
                        payload = cipher.decrypt(b'\x00' * 4 + counter.to_bytes(8, 'little'), body, head)
                        counter += 1
                        if pending_configuration:
                            output.write(pending_configuration)
                            pending_configuration.clear()
                            state['pending_configuration_bytes'] = 0
                        offset = 0
                        while offset < len(payload):
                            n = struct.unpack_from('>I', payload, offset)[0]
                            offset += 4
                            assert n > 0 and offset + n <= len(payload)
                            output.write(b'\x00\x00\x00\x01' + payload[offset:offset + n])
                            offset += n
                        state['frames'] += 1
                        if self.split_streams:
                            state.setdefault('frame_received_times', []).append(time.monotonic())
                        output.flush()
        except EOFError:
            pass
        except Exception as error:
            self.errors.append('video:' + repr(error))

    def close(self):
        started = time.monotonic()
        with self.lock:
            self.closed = True
        self.listen.close()
        self.thread.join(1)
        with self.lock:
            workers = list(self.threads)
        for worker in workers:
            worker.join(max(0, started + 2 - time.monotonic()))
        with self.lock:
            connections = list(self.sockets)
        for connection in connections:
            try:
                connection.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            connection.close()
        for worker in [self.thread, *workers]:
            worker.join(max(0, started + 5 - time.monotonic()))
        if any(worker.is_alive() for worker in [self.thread, *workers]):
            raise RuntimeError('receiver workers did not stop')
