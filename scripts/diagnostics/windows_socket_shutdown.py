"""One Windows loopback-only duplication/shutdown observation; no app imports."""

import json
import os
import platform
import socket
import sys
import threading
import time

TIMEOUT_SECONDS = 3
TOTAL_SECONDS = 60
START = time.monotonic()
OUTPUT_LOCK = threading.Lock()


def emit(event, **fields):
    with OUTPUT_LOCK:
        print(json.dumps({"event": event, "elapsed": time.monotonic() - START,
                          **fields}, sort_keys=True), flush=True)


def failure(error):
    return {"type": type(error).__name__, "errno": getattr(error, "errno", None),
            "winerror": getattr(error, "winerror", None), "message": str(error)}


def bounded_exit():
    # An unexpectedly stuck Winsock call must not leave the job running forever.
    os.write(2, b'{"event":"total_deadline_exceeded","explicit_joins_unconfirmed":true}\n')
    os._exit(124)


def describe(stream):
    def observe(operation):
        try:
            return operation()
        except OSError as error:
            return {"error": failure(error)}

    return {"handle": stream.fileno(), "object": id(stream),
            "local": observe(stream.getsockname), "peer": observe(stream.getpeername),
            "python_timeout": stream.gettimeout(),
            "receive_timeout_ms": observe(lambda: stream.getsockopt(socket.SOL_SOCKET, socket.SO_RCVTIMEO)),
            "send_timeout_ms": observe(lambda: stream.getsockopt(socket.SOL_SOCKET, socket.SO_SNDTIMEO)),
            "inheritable": observe(stream.get_inheritable)}


def run_case(mode, order):
    name = mode + "/" + order
    streams = []
    reader = None
    read_result = []
    case_ok = False
    retirement_ok = False
    emit("case_begin", case=name, attempt=1)
    try:
        listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        streams.append(listener)
        listener.bind(("127.0.0.1", 0))
        listener.listen(1)
        listener.settimeout(TIMEOUT_SECONDS)
        original = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        streams.append(original)
        original.settimeout(TIMEOUT_SECONDS)
        original.connect(listener.getsockname())
        peer, _ = listener.accept()
        streams.append(peer)
        listener.close()
        # Real blocking Winsock recv with SO_RCVTIMEO, matching Rust's mechanism.
        # Python settimeout(3) would instead use nonblocking sockets plus select.
        for stream in (original, peer):
            stream.setblocking(True)
            stream.setsockopt(socket.SOL_SOCKET, socket.SO_RCVTIMEO, 3000)
            stream.setsockopt(socket.SOL_SOCKET, socket.SO_SNDTIMEO, 3000)
            if stream.getsockopt(socket.SOL_SOCKET, socket.SO_RCVTIMEO) != 3000:
                raise AssertionError("The original three-second receive bound was not installed")
        original.sendall(b"P")
        if peer.recv(1) != b"P":
            raise AssertionError("Original-to-peer establishment byte changed")
        peer.sendall(b"Q")
        if original.recv(1) != b"Q":
            raise AssertionError("Peer-to-original establishment byte changed")

        def clone(label):
            value = original.dup() if mode == "duplicate" else original
            if mode == "duplicate":
                streams.append(value)
            emit("handle_ready", case=name, role=label, value=describe(value))
            return value

        registry = clone("registry")
        writer = None
        if order != "before_relay_clone":
            writer = clone("relay_writer")

        if order == "after_read_entry":
            entered = threading.Event()

            def receive():
                emit("read_entry", case=name,
                     limit="Event precedes recv; kernel-blocked state is not observed")
                entered.set()
                try:
                    data = original.recv(1)
                    read_result.append({"kind": "return", "bytes": len(data)})
                except OSError as error:
                    read_result.append({"kind": "error", **failure(error)})
                emit("read_finished", case=name, result=read_result[-1])

            reader = threading.Thread(target=receive, name="original-socket-read")
            reader.start()
            if not entered.wait(TIMEOUT_SECONDS):
                raise AssertionError("Original read-entry event missing")
        emit("shutdown_begin", case=name, role="registry", value=describe(registry))
        try:
            registry.shutdown(socket.SHUT_RDWR)
            emit("shutdown_result", case=name, result="success", python_return=None)
        except OSError as error:
            emit("shutdown_result", case=name, result="error", **failure(error))
            raise
        # Production drains and drops only its registry handle; original and writer live.
        if mode == "duplicate":
            registry.close()
        registry = None
        emit("registry_dropped", case=name, original=describe(original))
        if order == "before_relay_clone":
            writer = clone("relay_writer_after_shutdown")
        emit("retained_original_and_writer", case=name,
             original_handle=original.fileno(), writer_handle=writer.fileno(),
             same_object=original is writer)
        started = time.monotonic()
        try:
            data = peer.recv(1)
            emit("peer_read", case=name, bytes=len(data),
                 duration=time.monotonic() - started)
            if data != b"":
                raise AssertionError("Peer received bytes instead of EOF")
        except OSError as error:
            emit("peer_read_error", case=name, duration=time.monotonic() - started,
                 **failure(error))
            raise
        if reader is not None:
            reader.join(TIMEOUT_SECONDS)
            if reader.is_alive():
                raise AssertionError("Original reader did not retire after shutdown")
            if read_result != [{"kind": "return", "bytes": 0}]:
                # A local canceled recv may error. The peer still must observe exact EOF.
                local_error = read_result[0] if len(read_result) == 1 else {}
                code = local_error.get("winerror") or local_error.get("errno")
                if local_error.get("kind") != "error" or code in (10060, 10035):
                    raise AssertionError("Unexpected original read result: " + repr(read_result))
        case_ok = True
    except Exception as error:
        emit("case_error", case=name, **failure(error))
    finally:
        unique = {id(stream): stream for stream in streams}
        for stream in unique.values():
            if stream.fileno() == -1:
                continue
            try:
                stream.shutdown(socket.SHUT_RDWR)
            except OSError as error:
                emit("cleanup_shutdown", case=name, **failure(error))
            try:
                stream.close()
            except OSError as error:
                emit("cleanup_close_error", case=name, **failure(error))
        if reader is not None:
            reader.join(TIMEOUT_SECONDS)
        retirement_ok = all(stream.fileno() == -1 for stream in unique.values())
        retirement_ok = retirement_ok and (reader is None or not reader.is_alive())
        emit("case_retirement", case=name, all_original_handles_closed=retirement_ok,
             reader_joined=reader is None or not reader.is_alive())
    emit("case_result", case=name, pass_eof=case_ok, retirement_ok=retirement_ok)
    return case_ok and retirement_ok


def main():
    if sys.platform != "win32":
        raise SystemExit("Refusing: this source-only probe requires actual Windows Winsock")
    if platform.python_implementation() != "CPython" or sys.version_info[:2] not in ((3, 12), (3, 13)):
        raise SystemExit("Refusing: reviewed CPython socket.dup source is limited to 3.12/3.13")
    emit("runtime", python=sys.version, implementation=platform.python_implementation(),
         executable=sys.executable, windows=platform.platform(),
         connection="127.0.0.1:ephemeral only", attempts_per_subcase=1,
         transport_timeout_seconds=TIMEOUT_SECONDS, total_seconds=TOTAL_SECONDS,
         limitations="Python dup overlaps Winsock APIs, not the Rust relay or proof of root cause")
    watchdog = threading.Timer(TOTAL_SECONDS, bounded_exit)
    watchdog.daemon = True
    watchdog.start()
    try:
        results = []
        for mode in ("duplicate", "shared_object"):
            for order in ("before_relay_clone", "after_relay_clone", "after_read_entry"):
                results.append(run_case(mode, order))
        emit("summary", passed=sum(results), failed=len(results) - sum(results),
             subcases=len(results), retries=0,
             conclusion="Transport observation only; historical relay failure remains unresolved")
        return 0 if all(results) else 1
    finally:
        if not any(t.name == "original-socket-read" and t.is_alive()
                   for t in threading.enumerate()):
            watchdog.cancel()
            watchdog.join()
        else:
            emit("reader_retirement_unconfirmed", watchdog_remains_armed=True)


if __name__ == "__main__":
    raise SystemExit(main())
