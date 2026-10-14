#!/usr/bin/env python3
"""Read public MCP through a normal host-launched agent, never private host IPC."""
import argparse
from contextlib import ExitStack
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import queue
import selectors
import subprocess
import threading
import time

MAX_STDERR_BYTES = 2 * 1024 * 1024
MAX_RPC_BYTES = 8 * 1024 * 1024

_spec = importlib.util.spec_from_file_location("observer_safe_io", Path(__file__).with_name("safe_io.py"))
safe_io = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(safe_io)


class Rpc:
    def __init__(self, binary, evidence):
        self.events = queue.Queue(maxsize=64)
        self.next_id = 0
        self.evidence = evidence
        self.failure = threading.Event()
        self.stop_readers = threading.Event()
        self.failure_reason = None
        self.stop_lock = threading.Lock()
        self.rpc_bytes = 0
        self.files = ExitStack()
        self.closed = False
        try:
            self.stderr = self.files.enter_context(safe_io.open_exclusive(evidence / "server.stderr"))
            self.rpc_log = self.files.enter_context(safe_io.open_exclusive(evidence / "rpc.jsonl"))
            self.process = subprocess.Popen(
                [str(binary), "--browser-mcp"], stdin=subprocess.PIPE,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=os.environ.copy(),
            )
        except BaseException:
            self.files.close()
            raise
        self.readers = [threading.Thread(target=reader, daemon=True) for reader in (self.read, self.read_stderr)]
        for reader in self.readers:
            reader.start()

    def check_failure(self):
        if self.failure.is_set():
            try:
                (self.evidence / "qualification.json").unlink(missing_ok=True)
            except OSError:
                pass
            raise RuntimeError(self.failure_reason)

    def stop_child(self):
        with self.stop_lock:
            if self.process.poll() is None:
                try:
                    self.process.terminate()
                except ProcessLookupError:
                    pass
                try:
                    self.process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    self.process.kill()
                    self.process.wait(timeout=3)

    def refuse(self, reason):
        self.failure_reason = reason
        self.failure.set()
        # An earlier successful response cannot qualify an observer that later overflows.
        try:
            (self.evidence / "qualification.json").unlink(missing_ok=True)
        except OSError:
            pass
        try:
            self.events.put_nowait(None)
        except queue.Full:
            pass
        self.stop_child()

    def read_stderr(self):
        length = 0
        try:
            descriptor = self.process.stderr.fileno()
            os.set_blocking(descriptor, False)
            with selectors.DefaultSelector() as poll:
                poll.register(descriptor, selectors.EVENT_READ)
                while not self.stop_readers.is_set():
                    if not poll.select(timeout=.1):
                        continue
                    chunk = os.read(descriptor, min(65536, MAX_STDERR_BYTES + 1 - length))
                    if not chunk:
                        return
                    remaining = MAX_STDERR_BYTES - length
                    self.stderr.write(chunk[:remaining])
                    self.stderr.flush()
                    length += len(chunk)
                    if length > MAX_STDERR_BYTES:
                        self.refuse("public MCP stderr exceeded byte cap")
                        return
        except OSError:
            self.refuse("public MCP stderr collection failed")

    def read(self):
        descriptor = self.process.stdout.fileno()
        os.set_blocking(descriptor, False)
        buffered = bytearray()
        with selectors.DefaultSelector() as poll:
            poll.register(descriptor, selectors.EVENT_READ)
            while not self.stop_readers.is_set():
                if not poll.select(timeout=.1):
                    continue
                chunk = os.read(descriptor, min(65536, 1024 * 1024 + 1 - len(buffered)))
                buffered.extend(chunk)
                if not chunk:
                    try:
                        self.events.put(None, timeout=1)
                    except queue.Full:
                        pass
                    return
                while b"\n" in buffered:
                    line, remainder = buffered.split(b"\n", 1)
                    buffered = bytearray(remainder)
                    try:
                        value = json.loads(line) if len(line) + 1 <= 1024 * 1024 else None
                        if not isinstance(value, dict):
                            value = None
                    except (ValueError, UnicodeError):
                        value = None
                    try:
                        self.events.put(value, timeout=1)
                    except queue.Full:
                        return
                    if value is None:
                        return
                if len(buffered) > 1024 * 1024:
                    try:
                        self.events.put(None, timeout=1)
                    except queue.Full:
                        pass
                    return

    def send(self, message):
        self.check_failure()
        self.process.stdin.write((json.dumps(message) + "\n").encode())
        self.process.stdin.flush()
        self.check_failure()

    def request(self, method, params):
        self.next_id += 1
        request_id = self.next_id
        self.send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
        deadline = time.monotonic() + 20
        while True:
            self.check_failure()
            message = self.events.get(timeout=max(.01, deadline - time.monotonic()))
            self.check_failure()
            if message is None:
                raise RuntimeError("public MCP transport stopped")
            if message.get("id") == request_id:
                encoded = (json.dumps({"at": time.time(), "method": method,
                                       "params": params, "response": message}) + "\n").encode()
                if self.rpc_bytes + len(encoded) > MAX_RPC_BYTES:
                    self.refuse("public MCP reply evidence exceeded byte cap")
                    self.check_failure()
                self.rpc_log.write(encoded)
                self.rpc_log.flush()
                self.rpc_bytes += len(encoded)
                return message
            if time.monotonic() >= deadline:
                raise RuntimeError("public MCP reply deadline exceeded")

    def close(self):
        if self.closed:
            self.check_failure()
            return
        if self.process.poll() is None:
            try:
                self.process.stdin.close()
            except BrokenPipeError:
                pass
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.stop_child()
        for reader in self.readers:
            reader.join(timeout=3)
        try:
            if any(reader.is_alive() for reader in self.readers):
                self.stop_readers.set()
                for reader in self.readers:
                    reader.join(timeout=1)
                self.refuse("public MCP collector did not stop")
            self.check_failure()
        finally:
            for pipe in (self.process.stdin, self.process.stdout, self.process.stderr):
                pipe.close()
            self.files.close()
            self.closed = True


def validate_launch(environment, home, binary, expected):
    if home != Path("/fixture/state/horizon") or not (home / ".task-1335-api-native-fixture").is_file():
        raise RuntimeError("private synthetic fixture root required")
    if environment.get("HORIZON_BROWSER_ROOT") not in (None, str(home)):
        raise RuntimeError("another public MCP root refused")
    if not environment.get("HORIZON_BROWSER_ACTOR", "").startswith("horizon:") or not environment.get("HORIZON_BROWSER_HOST_INSTANCE"):
        raise RuntimeError("launch observer through a normal Horizon agent panel")
    if len(expected) != 64 or hashlib.sha256(binary.read_bytes()).hexdigest() != expected:
        raise RuntimeError("frozen Horizon hash mismatch")


def public_payload(reply):
    result = reply.get("result", {})
    if "error" in reply or not isinstance(result, dict) or result.get("isError"):
        raise RuntimeError("public MCP call refused")
    structured = result.get("structuredContent")
    if isinstance(structured, dict):
        return structured
    for item in result.get("content", []):
        if item.get("type") == "text":
            value = json.loads(item["text"])
            if isinstance(value, dict):
                return value
    raise RuntimeError("public MCP object payload missing")


def validate_inventory(payload, expected, expect_api_status, initial):
    rows = payload.get("tailnets")
    if rows is None:
        return False
    if not isinstance(rows, list) or any(not isinstance(row, dict) or set(row) != {"id", "name"} for row in rows):
        raise RuntimeError("public legacy tailnet item shape mismatch")
    ids = [row["id"] for row in rows]
    if any(not isinstance(value, str) or value not in expected["names"] for value in ids) or len(set(ids)) != len(ids):
        raise RuntimeError("public tailnet identity mismatch")
    if any(not isinstance(row["name"], str) for row in rows):
        raise RuntimeError("public tailnet name mismatch")
    if initial and {row["id"]: row["name"] for row in rows} != expected["names"]:
        return False
    if not expect_api_status:
        return True
    observations = payload.get("tailnet_api_credentials")
    if not isinstance(observations, dict) or set(observations) != set(ids):
        if initial:
            return False
        raise RuntimeError("public API status map mismatch")
    ready = True
    for identity, observation in observations.items():
        if not isinstance(observation, dict) or observation.get("status") not in {"ready", "pending", "unavailable"}:
            raise RuntimeError("public API observation malformed")
        keys = {"status", "credential"} if observation["status"] == "ready" else {"status"}
        if set(observation) != keys:
            raise RuntimeError("public API observation fields mismatch")
        if observation["status"] != "ready":
            ready = False
            continue
        credential = observation.get("credential")
        if credential != expected["credentials"][identity]:
            raise RuntimeError("public API credential kind mismatch")
    return ready if initial else True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("candidate", type=Path)
    parser.add_argument("sha256")
    parser.add_argument("--seconds", type=int, default=1800)
    parser.add_argument("--expect-api-status", action="store_true")
    args = parser.parse_args()
    os.umask(0o077)
    home = Path(os.environ.get("HORIZON_HOME", ""))
    validate_launch(os.environ, home, args.candidate, args.sha256)
    evidence = home.parent / "mcp-evidence"
    evidence.mkdir(mode=0o700, exist_ok=True)
    expected = json.loads((home.parent / "expected-inputs.json").read_text())
    rpc = Rpc(args.candidate, evidence)
    try:
        reply = rpc.request("initialize", {"protocolVersion": "2025-03-26", "capabilities": {},
                                           "clientInfo": {"name": "synthetic-api-status", "version": "1"}})
        if "error" in reply:
            raise RuntimeError("public MCP initialization failed")
        rpc.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        reply = rpc.request("tools/list", {})
        names = {tool.get("name") for tool in reply.get("result", {}).get("tools", [])}
        if not {"cloud_list", "cloud_companions"} <= names:
            raise RuntimeError("public status tools missing")
        print("Public MCP observer ready; synthetic fixture only", flush=True)
        deadline = time.monotonic() + max(1, min(args.seconds, 3600))
        cycle = 0
        qualified = False
        bootstrap_deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            for name, arguments in [("cloud_list", {"operation": "list"}), ("cloud_companions", {})]:
                reply = rpc.request("tools/call", {"name": name, "arguments": arguments})
                payload = public_payload(reply)
                if name == "cloud_companions" and validate_inventory(payload, expected, args.expect_api_status, not qualified):
                    if not qualified:
                        rpc.check_failure()
                        qualified = True
                        (evidence / "qualification.json").write_text(json.dumps({"mode": "api_status" if args.expect_api_status else "baseline", "public_initialize_tools_list": True, "public_cloud_list": True, "public_legacy_three_tailnets": True, "public_api_status_initial": args.expect_api_status, "raw_responses": "rpc.jsonl", "at": time.time()}))
                if not qualified and time.monotonic() >= bootstrap_deadline:
                    raise RuntimeError("public fixture bootstrap qualification timed out")
            cycle += 1
            if cycle % 10 == 0:
                print(f"Public read cycle {cycle}; results retained separately", flush=True)
            time.sleep(1)
    finally:
        rpc.close()


if __name__ == "__main__":
    main()
