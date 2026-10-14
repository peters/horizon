"""The outbound allowlist of the worker container on this host. `proxy` takes every TCP
connection that the container opens to port 443, reads the name in the TLS ClientHello and
connects to that name itself when the allowlist has it. `dns` answers the container's DNS
queries for allowed names only. The firewall sends nothing else out, so when either service
stops, the container loses that way out; it never gains another. Refused names go to a
root-owned log on the volume, at most one line per name and minute."""
import ipaddress
import json
import os
import socket
import stat
import struct
import sys
import threading
import time

ALLOW = '/etc/horizon-worker/egress-allow'
GATEWAY = '172.30.0.1'
PROXY_PORT = 3129
UPSTREAM = ('127.0.0.53', 53)
LOG_DIRECTORY = '/mnt/horizon-volume/workspace/.horizon-egress'
LOG = 'refused.jsonl'
LOG_LIMIT = 256 << 10
HELLO_LIMIT = 16 << 10
HELLO_SECONDS = 10
IDLE_SECONDS = 1800
# How often a quiet direction looks at the other one; also the longest wait for a write.
IDLE_CHECK = 300
CONNECTIONS = 256
# Linux IP_FREEBIND: listen on the bridge address before Docker makes the bridge.
FREEBIND = 15


def load(path=ALLOW):
    names, suffixes = set(), set()
    with open(path, encoding='ascii') as lines:
        for line in lines:
            line = line.strip().lower()
            if line.startswith('*.'):
                suffixes.add(line[2:])
            elif line:
                names.add(line)
    return names, suffixes


def valid_name(name):
    labels = name.split('.')
    return 0 < len(name) <= 253 and len(labels) > 1 and all(
        0 < len(label) <= 63 and label[0] != '-' and label[-1] != '-'
        and all(c.isascii() and (c.isalnum() or c in '-_') for c in label) for label in labels)


def allowed(allow, name):
    """Whether `name`, in any case and with or without its last dot, is on the allowlist."""
    names, suffixes = allow
    name = name.lower().removesuffix('.')
    if not valid_name(name):
        return False
    return name in names or any(name.endswith('.' + suffix) for suffix in suffixes)


class Log:
    """Refused names, one JSON line each, at most once per name and minute and 30 a minute."""

    def __init__(self, directory=LOG_DIRECTORY, now=time.time):
        self.directory, self.now, self.lock = directory, now, threading.Lock()
        self.seen, self.minute, self.count = {}, 0, 0

    def refused(self, name, via):
        name = name.lower().removesuffix('.') if valid_name(name.lower().removesuffix('.')) else '-'
        with self.lock:
            now = int(self.now())
            if now - self.seen.get(name, -60) < 60:
                return
            if now // 60 != self.minute:
                self.minute, self.count = now // 60, 0
                self.seen = {key: at for key, at in self.seen.items() if now - at < 60}
            if self.count >= 30:
                return
            self.seen[name], self.count = now, self.count + 1
            try:
                self.write(json.dumps({'at': now, 'name': name, 'via': via}) + '\n')
            except OSError:
                pass

    def write(self, line):
        # Never through a link, and only into a folder that this service's account alone
        # (root) can change.
        folder = os.open(self.directory, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            info = os.fstat(folder)
            if info.st_uid != os.geteuid() or info.st_mode & 0o022:
                return
            # Without waiting on a pipe, and only into a regular file.
            flags = os.O_WRONLY | os.O_APPEND | os.O_CREAT | os.O_NOFOLLOW | os.O_NONBLOCK
            descriptor = os.open(LOG, flags, 0o644, dir_fd=folder)
            try:
                info = os.fstat(descriptor)
                if not stat.S_ISREG(info.st_mode):
                    return
                if info.st_size > LOG_LIMIT:
                    os.ftruncate(descriptor, 0)
                os.write(descriptor, line.encode())
            finally:
                os.close(descriptor)
        finally:
            os.close(folder)


def server_name(hello):
    """The host name of a complete TLS ClientHello handshake message, or None."""
    if len(hello) < 4 or hello[0] != 1:
        return None
    end = 4 + int.from_bytes(hello[1:4], 'big')
    at = 4 + 2 + 32
    at += 1 + hello[at]
    at += 2 + int.from_bytes(hello[at:at + 2], 'big')
    at += 1 + hello[at]
    extensions_end = at + 2 + int.from_bytes(hello[at:at + 2], 'big')
    at += 2
    if extensions_end > end or end > len(hello):
        return None
    while at + 4 <= extensions_end:
        kind, length = struct.unpack('!HH', hello[at:at + 4])
        at += 4
        if kind == 0:
            names = hello[at + 2:at + length]
            if len(names) >= 3 and names[0] == 0:
                size = int.from_bytes(names[1:3], 'big')
                try:
                    return names[3:3 + size].decode('ascii') if size == len(names) - 3 else None
                except UnicodeDecodeError:
                    return None
            return None
        at += length
    return None


def read_hello(connection):
    """(every byte read, the handshake message) from the first TLS records of `connection`.
    The message is None when the records are not a handshake or grow past HELLO_LIMIT."""
    data, handshake, at = b'', b'', 0
    while True:
        if len(handshake) >= 4 and len(handshake) >= 4 + int.from_bytes(handshake[1:4], 'big'):
            return data, handshake
        while len(data) < at + 5 or len(data) < at + 5 + int.from_bytes(data[at + 3:at + 5], 'big'):
            if len(data) > HELLO_LIMIT or len(data) > at and data[at] != 0x16:
                return data, None
            chunk = connection.recv(4096)
            if not chunk:
                return data, None
            data += chunk
        if data[at] != 0x16:
            return data, None
        length = int.from_bytes(data[at + 3:at + 5], 'big')
        handshake += data[at + 5:at + 5 + length]
        at += 5 + length


def global_addresses(name, resolve=socket.getaddrinfo):
    """The public addresses of `name`; never this host, a private network or the metadata service."""
    found = []
    for family, kind, protocol, _, address in resolve(name, 443, type=socket.SOCK_STREAM):
        if ipaddress.ip_address(address[0].split('%')[0]).is_global:
            found.append((family, kind, protocol, address))
    return found


def pump(source, target, active):
    """Copies `source` to `target` until it ends, or until neither direction of the
    connection carried data for IDLE_SECONDS: a client that waits for a long answer
    keeps its connection while the answer arrives."""
    try:
        while True:
            try:
                chunk = source.recv(65536)
            except TimeoutError:
                if time.monotonic() - active[0] < IDLE_SECONDS:
                    continue
                break
            if not chunk:
                break
            active[0] = time.monotonic()
            target.sendall(chunk)
    except OSError:
        pass
    for connection, how in ((target, socket.SHUT_WR), (source, socket.SHUT_RD)):
        try:
            connection.shutdown(how)
        except OSError:
            pass


def connect(name, resolve=socket.getaddrinfo, port=443):
    """A connection to `name` on one of its public addresses, or None."""
    for family, kind, protocol, address in global_addresses(name, resolve):
        upstream = socket.socket(family, kind, protocol)
        try:
            upstream.settimeout(HELLO_SECONDS)
            upstream.connect((address[0], port) + tuple(address[2:]))
            return upstream
        except OSError:
            upstream.close()
    return None


def relay(client, allow, log, opener=connect):
    """Carries one connection of the container to the allowed name in its ClientHello."""
    upstream = None
    try:
        client.settimeout(HELLO_SECONDS)
        data, hello = read_hello(client)
        name = server_name(hello) if hello else None
        if name is None or not allowed(allow, name):
            log.refused(name or '-', 'tls')
            return
        upstream = opener(name)
        if upstream is None:
            return
        upstream.sendall(data)
        client.settimeout(IDLE_CHECK)
        upstream.settimeout(IDLE_CHECK)
        active = [time.monotonic()]
        back = threading.Thread(target=pump, args=(upstream, client, active), daemon=True)
        back.start()
        pump(client, upstream, active)
        back.join()
    except (OSError, ValueError, IndexError, UnicodeError, struct.error):
        pass
    finally:
        client.close()
        if upstream is not None:
            upstream.close()


def listener(kind, address):
    server = socket.socket(socket.AF_INET, kind)
    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    server.setsockopt(socket.SOL_IP, FREEBIND, 1)
    server.bind(address)
    if kind == socket.SOCK_STREAM:
        server.listen(128)
    return server


def spawn(slots, work, *arguments):
    """Runs `work` on its own thread while fewer than CONNECTIONS run; else drops it."""
    if not slots.acquire(blocking=False):
        return False

    def run():
        try:
            work(*arguments)
        finally:
            slots.release()
    threading.Thread(target=run, daemon=True).start()
    return True


def serve_proxy(server, allow, log, opener=connect):
    slots = threading.BoundedSemaphore(CONNECTIONS)
    while True:
        client, _ = server.accept()
        if not spawn(slots, relay, client, allow, log, opener):
            client.close()


def question(query):
    """The name of the one question of a DNS query, or None."""
    if len(query) < 12 or query[2] & 0x80 or int.from_bytes(query[4:6], 'big') != 1:
        return None
    labels, at = [], 12
    while at < len(query) and query[at]:
        length = query[at]
        if length > 63:
            return None
        labels.append(query[at + 1:at + 1 + length])
        at += 1 + length
    if at + 5 > len(query):
        return None
    try:
        return b'.'.join(labels).decode('ascii')
    except UnicodeDecodeError:
        return None


def refusal(query):
    """NXDOMAIN for `query`, with its question and no answers."""
    at = 12
    while at < len(query) and query[at]:
        at += 1 + query[at]
    flags = 0x8000 | (query[2] & 0x79) << 8 | 0x80 | 3
    return query[:2] + struct.pack('!HHHHH', flags, 1, 0, 0, 0) + query[12:at + 5]


def forward(query, upstream, tcp):
    """The upstream resolver's reply to `query`, over the transport the container used."""
    kind = socket.SOCK_STREAM if tcp else socket.SOCK_DGRAM
    with socket.socket(socket.AF_INET, kind) as resolver:
        resolver.settimeout(5)
        resolver.connect(upstream)
        if not tcp:
            resolver.send(query)
            return resolver.recv(65535)
        resolver.sendall(len(query).to_bytes(2, 'big') + query)
        size = int.from_bytes(resolver.recv(2, socket.MSG_WAITALL), 'big')
        return resolver.recv(size, socket.MSG_WAITALL)


def answer(query, allow, log, upstream=UPSTREAM, tcp=False):
    """The reply to the container's `query`: the resolver's for an allowed name, else NXDOMAIN."""
    name = question(query)
    if name is None:
        return None
    if not allowed(allow, name):
        log.refused(name, 'dns')
        return refusal(query)
    reply = forward(query, upstream, tcp)
    return reply if reply[:2] == query[:2] else None


def reply_udp(server, query, client, allow, log, upstream):
    try:
        reply = answer(query, allow, log, upstream)
        if reply:
            server.sendto(reply, client)
    except (OSError, ValueError, IndexError):
        pass


def reply_tcp(client, allow, log, upstream):
    with client:
        try:
            client.settimeout(HELLO_SECONDS)
            size = int.from_bytes(client.recv(2, socket.MSG_WAITALL), 'big')
            query = client.recv(size, socket.MSG_WAITALL)
            reply = answer(query, allow, log, upstream, tcp=True) if size and len(query) == size else None
            if reply:
                client.sendall(len(reply).to_bytes(2, 'big') + reply)
        except (OSError, ValueError, IndexError):
            pass


def serve_dns_udp(server, allow, log, upstream=UPSTREAM):
    slots = threading.BoundedSemaphore(CONNECTIONS)
    while True:
        query, client = server.recvfrom(65535)
        spawn(slots, reply_udp, server, query, client, allow, log, upstream)


def serve_dns_tcp(server, allow, log, upstream=UPSTREAM):
    slots = threading.BoundedSemaphore(CONNECTIONS)
    while True:
        client, _ = server.accept()
        if not spawn(slots, reply_tcp, client, allow, log, upstream):
            client.close()


def main(mode):
    allow, log = load(), Log()
    if mode == 'proxy':
        serve_proxy(listener(socket.SOCK_STREAM, (GATEWAY, PROXY_PORT)), allow, log)
    elif mode == 'dns':
        tcp = listener(socket.SOCK_STREAM, (GATEWAY, 53))
        threading.Thread(target=serve_dns_tcp, args=(tcp, allow, log), daemon=True).start()
        serve_dns_udp(listener(socket.SOCK_DGRAM, (GATEWAY, 53)), allow, log)
    else:
        raise SystemExit('usage: horizon-worker-egress proxy|dns')


if __name__ == '__main__':
    main(sys.argv[1] if len(sys.argv) == 2 else '')
