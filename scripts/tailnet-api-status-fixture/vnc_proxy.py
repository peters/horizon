#!/usr/bin/env python3
"""Transport actual owned-container RFB bytes; never synthesize frames or input."""
import argparse
import json
import os
from pathlib import Path
import selectors
import signal
import socket
import subprocess
import threading
import time
import uuid

def guest():
    with socket.create_connection(("127.0.0.1", 5900), timeout=5) as connection:
        connection.setblocking(False)
        os.set_blocking(0, False)
        os.set_blocking(1, False)
        incoming, outgoing = bytearray(), bytearray()
        deadline = time.monotonic() + 1800
        with selectors.DefaultSelector() as poll:
            while time.monotonic() < deadline:
                for key in list(poll.get_map().values()):
                    poll.unregister(key.fileobj)
                if len(incoming) < 65536:
                    poll.register(0, selectors.EVENT_READ)
                events = (selectors.EVENT_READ if len(outgoing) < 65536 else 0) | (selectors.EVENT_WRITE if incoming else 0)
                if events:
                    poll.register(connection, events)
                if outgoing:
                    poll.register(1, selectors.EVENT_WRITE)
                for key, ready in poll.select(timeout=.2):
                    try:
                        if key.fileobj == 0:
                            data = os.read(0, 65536 - len(incoming))
                            if not data:
                                return
                            incoming.extend(data)
                        elif key.fileobj == 1:
                            del outgoing[:os.write(1, outgoing)]
                        else:
                            if ready & selectors.EVENT_READ:
                                data = connection.recv(65536 - len(outgoing))
                                if not data:
                                    return
                                outgoing.extend(data)
                            if ready & selectors.EVENT_WRITE:
                                del incoming[:connection.send(incoming)]
                    except BlockingIOError:
                        continue

def read_control(path):
    metadata = path.lstat()
    if path.is_symlink() or not path.is_file() or metadata.st_mode & 0o077 or metadata.st_uid != os.getuid() or metadata.st_size > 4096:
        raise RuntimeError("private relay control refused")
    with path.open("rb") as source:
        raw = source.read(4097)
    if len(raw) > 4096:
        raise RuntimeError("private relay control oversized")
    value = json.loads(raw)
    if not isinstance(value, dict):
        raise RuntimeError("private relay control shape refused")
    return value

def binding_for(ownership, record, nonce):
    if not isinstance(nonce, str) or len(nonce) != 32 or any(character not in "0123456789abcdef" for character in nonce):
        raise RuntimeError("relay nonce refused")
    return {"ownership": str(ownership.resolve()), "daemon_id": record["daemon_id"],
            "container_id": record["container_id"], "labels": record["labels"], "relay_nonce": nonce}

def write_exclusive(path, value):
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w") as output:
        output.write(json.dumps(value) + "\n")

def request_stop(ownership):
    from run import owned
    record = owned(ownership)
    state = ownership.parent / "state"
    binding = read_control(state / "vnc-relay-owner.json")
    if binding != binding_for(ownership, record, binding.get("relay_nonce")):
        raise RuntimeError("relay ownership mismatch")
    path = state / "vnc-stop-request.json"
    try:
        write_exclusive(path, binding)
    except FileExistsError:
        if read_control(path) != binding:
            raise RuntimeError("relay stop request belongs to another ownership")
    print(json.dumps({"stop_requested": True, "container_id": binding["container_id"],
                      "relay_nonce": binding["relay_nonce"]}), flush=True)

def stop_requested(state, binding):
    path = state / "vnc-stop-request.json"
    if not path.exists() and not path.is_symlink():
        return False
    if read_control(path) != binding:
        raise RuntimeError("relay stop request belongs to another ownership")
    return True

def serve(ownership):
    from run import owned
    record = owned(ownership)
    container = record["container_id"]
    state = ownership.parent / "state"
    binding = binding_for(ownership, record, uuid.uuid4().hex)
    write_exclusive(state / "vnc-relay-owner.json", binding)
    active, children = [], []
    lock = threading.Lock()
    stop = threading.Event()
    signal.signal(signal.SIGTERM, lambda *_: stop.set())
    signal.signal(signal.SIGINT, lambda *_: stop.set())
    deadline = time.monotonic() + 1800
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        listener.listen(2)
        listener.settimeout(.2)
        (state / "vnc-address.json").write_text(json.dumps({"endpoint": f"127.0.0.1:{listener.getsockname()[1]}",
                                                           "actual_owned_vnc": True}))
        def relay(client):
            process = None
            try:
                owned(ownership)
                process = subprocess.Popen(["docker", "exec", "-i", container, "/usr/bin/python3",
                                            "/fixture/source/scripts/tailnet-api-status-fixture/vnc_proxy.py", "--guest"],
                                           stdin=client, stdout=client, stderr=subprocess.DEVNULL)
                with lock:
                    children.append(process)
                while process.poll() is None and not stop.wait(.2):
                    pass
            finally:
                try:
                    if process and process.poll() is None:
                        process.terminate()
                        try:
                            process.wait(timeout=3)
                        except subprocess.TimeoutExpired:
                            process.kill()
                            process.wait(timeout=3)
                finally:
                    client.close()
                    with lock:
                        active.remove(threading.current_thread())
        try:
            while not stop.is_set() and time.monotonic() < deadline:
                if stop_requested(state, binding):
                    stop.set()
                    break
                try:
                    client, _ = listener.accept()
                except socket.timeout:
                    continue
                with lock:
                    if len(active) >= 2:
                        client.close()
                        continue
                    worker = threading.Thread(target=relay, args=(client,))
                    active.append(worker)
                    worker.start()
        finally:
            stop.set()
            with lock:
                workers = list(active)
            for worker in workers:
                worker.join(timeout=8)
            (state / "vnc-address.json").unlink(missing_ok=True)
    stopped = all(not worker.is_alive() for worker in workers) and all(process.poll() is not None for process in children)
    (state / "vnc-proxy-result.json").write_text(json.dumps({"owned_relay_stopped": stopped,
                                                           "owned_children_stopped": all(process.poll() is not None for process in children),
                                                           "container_id": container, "relay_nonce": binding["relay_nonce"],
                                                           "actual_vnc_transport": True}))
    if not stopped:
        raise RuntimeError("owned relay children did not stop")

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--guest", action="store_true")
    parser.add_argument("--stop", action="store_true")
    parser.add_argument("--ownership", type=Path)
    args = parser.parse_args()
    os.umask(0o077)
    if args.guest and (args.stop or args.ownership):
        parser.error("guest cannot request host relay control")
    if args.guest:
        guest()
    elif args.ownership:
        request_stop(args.ownership) if args.stop else serve(args.ownership)
    else:
        parser.error("guest or exact owned container ownership required")


if __name__ == "__main__":
    main()
