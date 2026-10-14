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


def serve(ownership):
    from run import owned
    record = owned(ownership)
    container = record["container_id"]
    state = ownership.parent / "state"
    active = []
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
                while process.poll() is None and not stop.wait(.2):
                    pass
            finally:
                if process and process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=3)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait(timeout=3)
                client.close()
                with lock:
                    active.remove(threading.current_thread())
        try:
            while not stop.is_set() and time.monotonic() < deadline:
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
    (state / "vnc-proxy-result.json").write_text(json.dumps({"owned_relay_stopped": all(not worker.is_alive() for worker in workers), "actual_vnc_transport": True}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--guest", action="store_true")
    parser.add_argument("--ownership", type=Path)
    args = parser.parse_args()
    if args.guest:
        guest()
    elif args.ownership:
        serve(args.ownership)
    else:
        parser.error("guest or exact owned container ownership required")


if __name__ == "__main__":
    main()
