"""Private SSH/PTY fixture support. Synthetic processes only; no provider calls."""
import base64
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import socket
import signal
import threading
import struct
import subprocess
import sys
import termios
import time

LIMIT = 65536


def frame(sock, kind, data=b""):
    assert len(data) <= LIMIT
    sock.sendall(kind + struct.pack("!I", len(data)) + data)


def receive(sock):
    def exact(size):
        value = b""
        while len(value) < size:
            part = sock.recv(size - len(value))
            if not part:
                raise EOFError()
            value += part
        return value
    header = exact(5)
    size = struct.unpack("!I", header[1:])[0]
    assert size <= LIMIT
    return header[:1], exact(size)


def size(fd, columns, rows):
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, columns, 0, 0))


def serve_terminal(connection, request):
    master, slave = pty.openpty()
    size(slave, *request["size"])
    command = ["/worker", "attach-project-session", request["encoded"]]
    racing = Path("/control/attachment-stop-race").exists()
    if racing:
        command = ["/usr/bin/strace", "-D", "-ff", "-o", "/control/attachment-race-trace", "-e", "trace=bind", "-e", "inject=bind:signal=SIGSTOP:when=1"] + command
    process = subprocess.Popen(command, stdin=slave, stdout=slave, stderr=slave, env={"PATH": "/usr/bin:/bin"})
    watcher = None
    if racing:
        def resume():
            deadline = time.monotonic() + 30
            while process.poll() is None and time.monotonic() < deadline:
                status = Path(f"/proc/{process.pid}/status").read_text()
                trace = Path(f"/control/attachment-race-trace.{process.pid}")
                injected = trace.exists() and "stopped by SIGSTOP" in trace.read_text()
                if injected and any(line.startswith("State:") and line.split()[1] in ("T", "t") for line in status.splitlines()):
                    Path("/control/attachment-bind-paused").write_text("paused after upstream connect")
                    break
                time.sleep(.01)
            while process.poll() is None and time.monotonic() < deadline:
                if Path("/control/attachment-continue").exists():
                    os.kill(process.pid, signal.SIGCONT)
                    return
                time.sleep(.01)
        watcher = threading.Thread(target=resume)
        watcher.start()
    os.close(slave)
    deadline = time.monotonic() + 150
    queued = bytearray()
    try:
        while time.monotonic() < deadline:
            ready, _, _ = select.select([connection, master], [], [], .1)
            if connection in ready:
                kind, data = receive(connection)
                if kind == b"I":
                    offset = 0
                    while offset < len(data):
                        offset += os.write(master, data[offset:])
                    if racing:
                        queued.extend(data)
                        assert len(queued) < 1024
                        if b"must-not-cross-stop-gate\n" in queued:
                            assert Path("/control/attachment-bind-paused").exists()
                            Path("/control/attachment-paused").write_text("user input queued in paused worker PTY")
                elif kind == b"W":
                    size(master, *struct.unpack("!II", data))
                else:
                    raise ValueError("Unknown terminal frame")
            if master in ready:
                try:
                    data = os.read(master, LIMIT)
                except OSError:
                    break
                if not data:
                    break
                frame(connection, b"O", data)
            if process.poll() is not None:
                break
        process.wait(timeout=3)
        frame(connection, b"X", struct.pack("!i", process.returncode))
    except (EOFError, BrokenPipeError, ConnectionResetError):
        pass
    finally:
        os.close(master)
        if process.poll() is None:
            process.kill()
        process.wait(timeout=5)
        if watcher is not None:
            watcher.join(timeout=2)
        connection.close()


def forward(channel, root, encoded, dimensions):
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(12)
        client.connect(str(root / "control/service.sock"))
        client.sendall(json.dumps({"command": "attach-project-session", "encoded": encoded, "size": dimensions()}).encode() + b"\n")
        previous = dimensions()
        deadline = time.monotonic() + 140
        while time.monotonic() < deadline:
            current = dimensions()
            if current != previous:
                frame(client, b"W", struct.pack("!II", *current))
                previous = current
            if channel.closed or channel.eof_received:
                return -1
            ready, _, _ = select.select([channel, client], [], [], .1)
            if channel in ready:
                data = channel.recv(LIMIT)
                if not data:
                    return -1
                frame(client, b"I", data)
            if client in ready:
                kind, data = receive(client)
                if kind == b"O":
                    channel.sendall(data)
                elif kind == b"X":
                    return struct.unpack("!i", data)[0]
                else:
                    raise ValueError("Unknown worker terminal frame")
        raise TimeoutError("Fixture terminal deadline expired")


class Client:
    def __init__(self, arguments, evidence):
        self.evidence = evidence
        self.master, slave = pty.openpty()
        size(slave, 90, 25)
        def controlling_terminal():
            os.setsid()
            fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
        self.process = subprocess.Popen(["ssh"] + arguments, stdin=slave, stdout=slave, stderr=slave, preexec_fn=controlling_terminal)
        os.close(slave)
        self.output = b""

    def write(self, value):
        os.write(self.master, value)

    def until(self, value, timeout=15):
        deadline = time.monotonic() + timeout
        while value not in self.output:
            assert time.monotonic() < deadline, "Terminal marker deadline expired"
            if select.select([self.master], [], [], .1)[0]:
                self.output += os.read(self.master, LIMIT)
                assert len(self.output) < 4 * 1024 * 1024
        return self.output

    def exited(self, timeout=15):
        deadline = time.monotonic() + timeout
        while self.process.poll() is None:
            assert time.monotonic() < deadline, "SSH client did not exit"
            if select.select([self.master], [], [], .05)[0]:
                try:
                    self.output += os.read(self.master, LIMIT)
                except OSError:
                    pass
        return self.process.returncode

    def close(self):
        (self.evidence / f"terminal-client-{self.process.pid}.bin").write_bytes(self.output)
        if self.master is not None:
            os.close(self.master)
            self.master = None
        if self.process.poll() is None:
            self.process.terminate()
        self.process.wait(timeout=5)


def clients(path):
    settings = json.loads(Path(path).read_text())
    root = Path(settings["directory"])
    active = []
    try:
        rejected = list(settings["arguments"][0])
        prefix, encoded = rejected[-1].rsplit(" ", 1)
        request = json.loads(bytes.fromhex(encoded))
        request["payload"] += " "
        rejected[-1] = prefix + " " + json.dumps(request).encode().hex()
        client = Client(rejected, root)
        active.append(client)
        assert client.exited() == 1 and b"interactive-ready" not in client.output
        client.close()
        active.clear()
        for index, arguments in enumerate(settings["arguments"]):
            client = Client(arguments, root)
            active.append(client)
            marker = f"session-{index}-first".encode()
            client.until(b"interactive-ready")
            client.write(marker + b"\n")
            client.until(b"echo:" + marker)
        # A second same-owner client must neither launch nor detach the first.
        second = Client(settings["arguments"][0], root)
        active.append(second)
        second.until(b"interactive-ready")
        second.write(b"second-client\n")
        second.until(b"echo:second-client")
        active[0].until(b"echo:second-client")
        second.close()
        active[0].close()
        reconnected = Client(settings["arguments"][0], root)
        active.append(reconnected)
        reconnected.until(b"interactive-ready")
        reconnected.write(b"reconnected-same-process\n")
        reconnected.until(b"echo:reconnected-same-process")
        size(reconnected.master, 101, 31)
        reconnected.write(b"resized\n")
        reconnected.until(b"echo:resized")
        (root / "attachment-ready").write_text("ready")
        deadline = time.monotonic() + 45
        while not (root / "attachment-exited").exists():
            assert time.monotonic() < deadline
            time.sleep(.05)
        if settings.get("race", False):
            (root / "control/attachment-stop-race").touch()
            started = time.monotonic()
            racing = Client(settings["arguments"][0], root)
            active.append(racing)
            while not (root / "control/attachment-bind-paused").exists():
                assert time.monotonic() - started < 5
                time.sleep(.01)
            racing.write(b"must-not-cross-stop-gate\n")
            assert racing.exited(timeout=7) == 1 and b"interactive-ready" not in racing.output
            elapsed = time.monotonic() - started
            assert elapsed < 8, "rejection must precede the worker's 10-second handoff deadline"
            (root / "attachment-race-proof.json").write_text(json.dumps({"elapsed":elapsed,"input_queued":True,"rejected_before_deadline":True}))
            (root / "control/attachment-stop-race").unlink()
        else:
            reconnected.close()
            reconnected = Client(settings["arguments"][0], root)
            active.append(reconnected)
            reconnected.until(b"echo:reconnected-same-process")
            (root / "attachment-dead").write_text("retained dead pane")
        deadline = time.monotonic() + 90
        while not (root / "attachment-stopped").exists():
            assert time.monotonic() < deadline
            time.sleep(.05)
        reconnected.process.wait(timeout=15)
        replay = Client(settings["arguments"][0], root)
        active.append(replay)
        assert replay.exited() == 1 and b"interactive-ready" not in replay.output
        # A sibling still accepts input after the explicit stop.
        active[5].write(b"sibling-after-stop\n")
        active[5].until(b"echo:sibling-after-stop")
        (root / "attachment-result.json").write_text(json.dumps({"passed": True, "clients": len(active), "reconnect": True, "stop_closed": True}))
    finally:
        for client in active:
            client.close()


if __name__ == "__main__":
    clients(sys.argv[1])
