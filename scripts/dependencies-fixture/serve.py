#!/usr/bin/env python3
"""Strict-key, loopback-only SSH fixture for the synthetic maintenance worker."""
import argparse
import base64
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import threading
import time

import paramiko
from worker import MAX_POLICY_BYTES, atomic_json, configure, initial, load_policy


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, required=True, help="Private data folder outside the repository")
    parser.add_argument("--delay", type=float, default=3)
    options = parser.parse_args()
    root = options.root.resolve()
    root.mkdir(parents=True, exist_ok=True)
    root.chmod(0o700)
    initial(root)
    for name in ("client_key", "host_key"):
        path = root / name
        if not path.exists():
            subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "local-maintenance-fixture", "-f", str(path)], check=True)
    host_key = paramiko.Ed25519Key.from_private_key_file(str(root / "host_key"))
    allowed = base64.b64decode((root / "client_key.pub").read_text().split()[1])
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen(8)
    listener.settimeout(0.5)
    port = listener.getsockname()[1]
    known = root / "known_hosts"
    known.write_text(f"[127.0.0.1]:{port} ssh-ed25519 {host_key.get_base64()}\n")
    known.chmod(0o600)
    ssh = ["-F", "/dev/null", "-i", str(root / "client_key"), "-p", str(port),
           "-o", "BatchMode=yes", "-o", "IdentitiesOnly=yes", "-o", "StrictHostKeyChecking=yes",
           "-o", f"UserKnownHostsFile={known}", "-o", "ConnectTimeout=3", "worker@127.0.0.1"]
    atomic_json(root / "connection.json", {"host": "127.0.0.1", "port": port, "username": "worker",
        "host_key_checking": "strict", "identity_file": str(root / "client_key"), "known_hosts": str(known),
        "ssh_args": ssh, "status_command": ["ssh"] + ssh + ["maintenance status"],
        "run_command": ["ssh", "-tt"] + ssh + ["maintenance run"],
        "start_command": ["ssh"] + ssh + ["maintenance start"],
        "configure_command": ["ssh"] + ssh + ["maintenance configure"],
        "stop_command": ["ssh"] + ssh + ["maintenance stop"],
        "diagnose_command": ["ssh"] + ssh + ["maintenance diagnose"],
        "watch_command": ["ssh", "-tt"] + ssh + ["maintenance watch"],
        "worker_root": str(root), "synthetic": True, "server_pid": os.getpid()})
    stop = threading.Event()
    lock = threading.Lock()
    process = [None]
    log_handle = [None]
    stopped_intentionally = [False]
    channels = []

    def start_worker():
        with lock:
            if process[0] is not None and process[0].poll() is None:
                return process[0], False
            if log_handle[0]:
                log_handle[0].close()
            log_handle[0] = (root / "worker.log").open("w")
            (root / "events.jsonl").write_text("")
            process[0] = subprocess.Popen([sys.executable, str(Path(__file__).with_name("worker.py")),
                "--root", str(root), "--delay", str(options.delay), "--stay-alive"], stdin=subprocess.DEVNULL,
                stdout=log_handle[0], stderr=subprocess.STDOUT, start_new_session=True)
            stopped_intentionally[0] = False
            return process[0], True

    def stop_worker():
        with lock:
            child = process[0]
            stopped_intentionally[0] = True
            if child is not None and child.poll() is None:
                # This process group belongs only to the fixture worker and its checks.
                os.killpg(child.pid, signal.SIGTERM)
                child.wait(timeout=5)
            return {"stopped": True, "pid": child.pid if child else None, "synthetic": True}

    def observed_status():
        status = json.loads((root / "status.json").read_text())
        status["configured_revision"] = load_policy(root)["revision"]
        beat = status.pop("_heartbeat_monotonic", None)
        health = status.get("worker_health", {})
        child = process[0]
        code = child.poll() if child is not None else None
        alive = child is not None and code is None
        age = max(0, time.monotonic() - beat) if isinstance(beat, (int, float)) else None
        health.update({"alive": alive, "pid": child.pid if child else None, "heartbeat_age_seconds": age})
        if child is None:
            health.update(state="not_started", last_error=None)
        elif not alive:
            health.update(state="stopped" if stopped_intentionally[0] else "error",
                          last_error=None if stopped_intentionally[0] else f"Worker process exited ({code})")
        elif age is None:
            health.update(state="starting", last_error=None)
        elif age > 6:
            health.update(state="unresponsive", last_error="Worker heartbeat is stale")
        status["worker_health"] = health
        return status

    def read_configuration(channel):
        channel.settimeout(5)
        data = bytearray()
        try:
            while True:
                chunk = channel.recv(min(4096, MAX_POLICY_BYTES + 1 - len(data)))
                if not chunk:
                    break
                data.extend(chunk)
                if len(data) > MAX_POLICY_BYTES:
                    raise ValueError("Configuration payload exceeds 65536 bytes")
        except socket.timeout as error:
            raise ValueError("Configuration input did not finish within five seconds") from error
        return json.loads(data)

    class Server(paramiko.ServerInterface):
        def __init__(self):
            self.executing = threading.Event()
            self.command = None

        def get_allowed_auths(self, username):
            return "publickey"

        def check_auth_publickey(self, username, key):
            return paramiko.AUTH_SUCCESSFUL if username == "worker" and key.asbytes() == allowed else paramiko.AUTH_FAILED

        def check_channel_request(self, kind, channel_id):
            return paramiko.OPEN_SUCCEEDED if kind == "session" else paramiko.OPEN_FAILED_ADMINISTRATIVELY_PROHIBITED

        def check_channel_pty_request(self, channel, term, width, height, pixelwidth, pixelheight, modes):
            return True

        def check_channel_window_change_request(self, *args):
            return True

        def check_channel_exec_request(self, channel, command):
            try:
                command = command.decode("ascii")
            except UnicodeDecodeError:
                return False
            if command not in ("maintenance status", "maintenance start", "maintenance run", "maintenance watch",
                               "maintenance configure", "maintenance stop", "maintenance diagnose"):
                return False
            self.command = command
            self.executing.set()
            return True

    def handle(client):
        transport = paramiko.Transport(client)
        channels.append(transport)
        try:
            transport.add_server_key(host_key)
            server = Server()
            transport.start_server(server=server)
            channel = transport.accept(timeout=5)
            if channel is None or not server.executing.wait(5):
                return
            command = server.command
            exit_code = 0
            if command == "maintenance status":
                channel.sendall((json.dumps(observed_status()) + "\n").encode())
            elif command == "maintenance configure":
                try:
                    request = read_configuration(channel)
                    with lock:
                        response = configure(root, request)
                except (ValueError, TypeError, UnicodeDecodeError, RecursionError) as error:
                    response = {"accepted": False, "error": str(error)}
                    exit_code = 2
                channel.sendall((json.dumps(response) + "\n").encode())
            elif command == "maintenance start":
                child, created = start_worker()
                channel.sendall((json.dumps({"started": created, "pid": child.pid, "synthetic": True}) + "\n").encode())
            elif command == "maintenance stop":
                channel.sendall((json.dumps(stop_worker()) + "\n").encode())
            elif command == "maintenance diagnose":
                log = root / "worker.log"
                # Only fixed fixture records are returned; key files are never read.
                tail = log.read_bytes()[-8192:].decode(errors="replace") if log.exists() else ""
                response = {"synthetic": True, "scope": "task-owned loopback maintenance fixture",
                            "status": observed_status(), "policy": load_policy(root), "log_tail": tail}
                channel.sendall((json.dumps(response) + "\n").encode())
            else:
                if command == "maintenance run":
                    child, _ = start_worker()
                else:
                    child = process[0]
                channel.sendall(b"Maintenance worker | loopback SSH | synthetic repositories\r\n")
                position = 0
                while not stop.is_set() and not channel.closed:
                    path = root / "worker.log"
                    if path.exists():
                        data = path.read_bytes()
                        if len(data) > position:
                            channel.sendall(data[position:].replace(b"\n", b"\r\n"))
                            position = len(data)
                    if child is not None and child.poll() is not None:
                        break
                    if command == "maintenance watch" and child is None:
                        break
                    time.sleep(0.2)
            channel.send_exit_status(exit_code)
            channel.shutdown_write()
            time.sleep(0.1)
        except (EOFError, OSError, paramiko.SSHException):
            pass
        finally:
            transport.close()

    def shutdown(_signal, _frame):
        stop.set()

    signal.signal(signal.SIGTERM, shutdown)
    signal.signal(signal.SIGINT, shutdown)
    print(json.dumps({"ready": True, "root": str(root), "port": port, "connection": str(root / "connection.json")}), flush=True)
    try:
        while not stop.is_set():
            try:
                client, address = listener.accept()
            except socket.timeout:
                continue
            if address[0] != "127.0.0.1":
                client.close()
                continue
            threading.Thread(target=handle, args=(client,), daemon=True).start()
    finally:
        listener.close()
        for transport in channels:
            transport.close()
        stop_worker()
        if log_handle[0]:
            log_handle[0].close()


if __name__ == "__main__":
    main()
