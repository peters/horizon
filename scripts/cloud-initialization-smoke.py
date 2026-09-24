#!/usr/bin/env python3
"""Real SSH initialization, recovery, key restart and abandonment smoke. Requires paramiko, OpenSSH and bubblewrap.

Run from the candidate checkout with its isolated Cargo target directory:
  python scripts/cloud-initialization-smoke.py --worker /frozen/horizon-cloud-worker \
    --evidence /absolute/new/private-directory
The host fixture seeds synthetic provider ownership; no provider API is used.
The actual worker creates its own bootstrap state on an isolated empty mount.
"""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import threading

import paramiko

LIMIT = 64 * 1024
COMMANDS = {b"horizon-cloud-worker " + name: name.decode() for name in
            [b"initialize-allocation", b"recover-allocation", b"abandon-bootstrap"]}
COMMANDS[b"cat /run/sshd/horizon-allocation/runtime.json"] = "runtime"


def run(options):
    os.umask(0o077)
    root = Path(options.evidence)
    if not root.is_absolute():
        raise ValueError("Evidence directory must be absolute and new")
    root.mkdir(mode=0o700, parents=True, exist_ok=False)
    (root / "workspace").mkdir(mode=0o700)
    shutil.copyfile(options.worker, root / "worker")
    (root / "worker").chmod(0o700)
    worker_hash = hashlib.sha256((root / "worker").read_bytes()).hexdigest()
    subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "recovery-fixture", "-f", str(root / "id_ed25519")], check=True)
    allowed = base64.b64decode((root / "id_ed25519.pub").read_text().split()[1])
    (root / "run").mkdir(mode=0o700)
    server_lock = threading.Lock()
    host_keys = []
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.settimeout(0.2)
    (root / "fixture.json").write_text(json.dumps({"port": listener.getsockname()[1], "worker_sha256": worker_hash}))
    stop = threading.Event()
    sessions = []
    errors = []
    children = []

    class Server(paramiko.ServerInterface):
        def __init__(self):
            self.executing = threading.Event()
            self.command = None

        def get_allowed_auths(self, username):
            return "publickey"

        def check_auth_publickey(self, username, key):
            if username == "root" and key.asbytes() == allowed:
                return paramiko.AUTH_SUCCESSFUL
            return paramiko.AUTH_FAILED

        def check_channel_request(self, kind, chanid):
            return paramiko.OPEN_SUCCEEDED if kind == "session" else paramiko.OPEN_FAILED_ADMINISTRATIVELY_PROHIBITED

        def check_channel_exec_request(self, channel, command):
            if command not in COMMANDS:
                return False
            self.command = COMMANDS[command]
            self.executing.set()
            return True

    def worker(command, request=b"", startup=False):
        arguments = ["bwrap", "--unshare-all", "--die-with-parent", "--new-session",
                     "--ro-bind", "/usr", "/usr", "--ro-bind", "/lib", "/lib", "--ro-bind", "/lib64", "/lib64",
                     "--proc", "/proc", "--dev", "/dev", "--dir", "/tmp",
                     "--ro-bind", "/etc/passwd", "/etc/passwd", "--ro-bind", "/etc/group", "/etc/group",
                     "--bind", str(root / "workspace"), "/workspace", "--bind", str(root / "run"), "/run/sshd", "--clearenv",
                     "--setenv", "PATH", "/usr/bin:/bin", "--setenv", "HOME", "/tmp"]
        if startup:
            runtime = json.loads((root / "runtime.json").read_text())
            for key, value in runtime.items():
                arguments.extend(["--setenv", key, value])
        arguments.extend(["--ro-bind", str(root / "worker"), "/worker", "/worker", command])
        return subprocess.run(arguments, input=request, capture_output=True, timeout=15)

    def handle(sock):
        transport = paramiko.Transport(sock)
        try:
            with server_lock:
                if (root / "restart").exists():
                    shutil.rmtree(root / "run")
                    (root / "run").mkdir(mode=0o700)
                    (root / "restart").unlink()
                prepared = worker("prepare-allocation-ssh", startup=True)
                if prepared.returncode:
                    raise RuntimeError("Worker startup preparation failed: " + prepared.stderr.decode())
                host_key = paramiko.Ed25519Key.from_private_key_file(str(root / "run/horizon-allocation/ssh-host-key"))
                host_keys.append(host_key.get_base64())
            transport.add_server_key(host_key)
            server = Server()
            transport.start_server(server=server)
            channel = transport.accept(10)
            if channel is None or not server.executing.wait(10):
                raise RuntimeError("No allowed SSH command received")
            channel.settimeout(10)
            request = bytearray()
            while True:
                chunk = channel.recv(4096)
                if not chunk:
                    break
                request.extend(chunk)
                if len(request) > LIMIT:
                    raise RuntimeError("SSH request exceeded its bound")
            if server.command == "runtime":
                result = subprocess.CompletedProcess([], 0, (root / "run/horizon-allocation/runtime.json").read_bytes(), b"")
            else:
                result = worker(server.command, bytes(request))
            if len(result.stdout) > LIMIT or len(result.stderr) > LIMIT:
                raise RuntimeError("Worker output exceeded its bound")
            sessions.append({"command": server.command, "request_sha256": hashlib.sha256(request).hexdigest(), "worker_sha256": worker_hash, "exit_code": result.returncode})
            channel.sendall(result.stdout)
            channel.send_exit_status(result.returncode)
            channel.shutdown_write()
            channel.close()
        except Exception as error:
            errors.append(type(error).__name__ + ": " + str(error))
        finally:
            transport.close()
            sock.close()

    def accept():
        # Advertise the reserved port before sshd is ready. The owning host must
        # retry read-only enrollment while retaining this live creation attempt.
        while not (root / "runtime.json").exists():
            if stop.wait(0.05):
                return
        if stop.wait(1):
            return
        listener.listen(4)
        while not stop.is_set():
            try:
                sock, _ = listener.accept()
            except socket.timeout:
                continue
            child = threading.Thread(target=handle, args=(sock,))
            children.append(child)
            child.start()

    thread = threading.Thread(target=accept)
    thread.start()
    test_exit = None
    try:
        environment = dict(os.environ, HORIZON_INITIALIZATION_FIXTURE=str(root))
        with open(root / "test.log", "w") as output:
            test_exit = subprocess.run(["cargo", "test", "-p", "horizon-core", "native_ssh_worker_initialization", "--lib", "--", "--ignored", "--nocapture"], env=environment, stdout=output, stderr=subprocess.STDOUT, timeout=300).returncode
    except (subprocess.TimeoutExpired, OSError) as error:
        errors.append(type(error).__name__ + ": " + str(error))
    finally:
        stop.set()
        thread.join(timeout=5)
        listener.close()
        for child in children:
            child.join(timeout=25)
        (root / "id_ed25519").unlink(missing_ok=True)
        for path in [root / "run/horizon-allocation/ssh-host-key", root / "workspace/.horizon-allocation/ssh-host-key"]:
            path.unlink(missing_ok=True)
    report = {"worker_sha256": worker_hash, "test_exit": test_exit, "same_host_key": len(set(host_keys)) == 1, "sessions": sessions, "errors": errors, "threads_stopped": not thread.is_alive() and all(not child.is_alive() for child in children)}
    (root / "ssh-report.json").write_text(json.dumps(report, indent=2))
    assert test_exit == 0 and not errors and report["threads_stopped"], "Inspect private test.log and ssh-report.json"
    assert [session["exit_code"] for session in sessions] == [0, 0, 0, 0, 0, 0, 0, 1, 1]
    assert report["same_host_key"]
    assert len({session["request_sha256"] for session in sessions if session["command"] == "recover-allocation"}) == 1
    assert len({session["request_sha256"] for session in sessions if session["command"] == "abandon-bootstrap"}) == 1
    print(json.dumps({"passed": True, "ssh_sessions": len(sessions), "same_host_key": True, "worker_sha256": worker_hash}))



if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--worker", required=True)
    parser.add_argument("--evidence", required=True)
    run(parser.parse_args())
