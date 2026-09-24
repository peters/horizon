#!/usr/bin/env python3
"""Real SSH + worker recovery smoke. Requires paramiko, OpenSSH and bubblewrap.

Run from the candidate checkout with its isolated Cargo target directory:
  python scripts/cloud-recovery-smoke.py --worker /frozen/horizon-cloud-worker \
    --evidence /absolute/new/private-directory
Only synthetic test records are seeded; this is not a production initializer.
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
REMOTE = b"horizon-cloud-worker recover-allocation"


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
    host_key = paramiko.RSAKey.generate(2048)
    (root / "known_hosts").write_text("horizon-cloud-worker1 " + host_key.get_name() + " " + host_key.get_base64() + "\n")
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen(4)
    listener.settimeout(0.2)
    (root / "fixture.json").write_text(json.dumps({"port": listener.getsockname()[1], "worker_sha256": worker_hash}))
    stop = threading.Event()
    sessions = []
    errors = []
    children = []

    class Server(paramiko.ServerInterface):
        def __init__(self):
            self.executing = threading.Event()

        def get_allowed_auths(self, username):
            return "publickey"

        def check_auth_publickey(self, username, key):
            if username == "root" and key.asbytes() == allowed:
                return paramiko.AUTH_SUCCESSFUL
            return paramiko.AUTH_FAILED

        def check_channel_request(self, kind, chanid):
            return paramiko.OPEN_SUCCEEDED if kind == "session" else paramiko.OPEN_FAILED_ADMINISTRATIVELY_PROHIBITED

        def check_channel_exec_request(self, channel, command):
            if command != REMOTE:
                return False
            self.executing.set()
            return True

    def handle(sock):
        transport = paramiko.Transport(sock)
        try:
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
            runtime = json.loads((root / "runtime.json").read_text())
            expected = {"HORIZON_WORKER_STARTUP", "RUNPOD_POD_ID", "RUNPOD_VOLUME_ID", "RUNPOD_DC_ID", "HORIZON_CLOUD_OPERATION"}
            if set(runtime) != expected or not all(isinstance(v, str) for v in runtime.values()):
                raise RuntimeError("Unexpected fixture environment")
            command = ["bwrap", "--unshare-all", "--die-with-parent", "--new-session",
                       "--ro-bind", "/usr", "/usr", "--ro-bind", "/lib", "/lib", "--ro-bind", "/lib64", "/lib64",
                       "--proc", "/proc", "--dev", "/dev", "--dir", "/tmp",
                       "--bind", str(root / "workspace"), "/workspace", "--clearenv",
                       "--setenv", "PATH", "/usr/bin:/bin", "--setenv", "HOME", "/tmp"]
            for key, value in runtime.items():
                command.extend(["--setenv", key, value])
            command.extend(["--ro-bind", str(root / "worker"), "/worker", "/worker", "recover-allocation"])
            result = subprocess.run(command, input=bytes(request), capture_output=True, timeout=10)
            if len(result.stdout) > LIMIT or len(result.stderr) > LIMIT:
                raise RuntimeError("Worker output exceeded its bound")
            sessions.append({"request_sha256": hashlib.sha256(request).hexdigest(), "worker_sha256": worker_hash, "exit_code": result.returncode})
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
    try:
        environment = dict(os.environ, HORIZON_RECOVERY_FIXTURE=str(root))
        with open(root / "test.log", "w") as output:
            test = subprocess.run(["cargo", "test", "-p", "horizon-core", "native_ssh_worker_recovery", "--lib", "--", "--ignored", "--nocapture"], env=environment, stdout=output, stderr=subprocess.STDOUT, timeout=300)
    finally:
        stop.set()
        thread.join(timeout=5)
        listener.close()
        for child in children:
            child.join(timeout=25)
        (root / "id_ed25519").unlink(missing_ok=True)
    report = {"worker_sha256": worker_hash, "test_exit": test.returncode, "sessions": sessions, "errors": errors, "threads_stopped": not thread.is_alive() and all(not child.is_alive() for child in children)}
    (root / "ssh-report.json").write_text(json.dumps(report, indent=2))
    assert test.returncode == 0 and not errors and report["threads_stopped"], "Inspect private test.log and ssh-report.json"
    assert [session["exit_code"] for session in sessions] == [0, 0, 0, 1]
    assert len({session["request_sha256"] for session in sessions}) == 1
    print(json.dumps({"passed": True, "ssh_sessions": len(sessions), "same_signed_request": True, "worker_sha256": worker_hash}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--worker", required=True)
    parser.add_argument("--evidence", required=True)
    run(parser.parse_args())
