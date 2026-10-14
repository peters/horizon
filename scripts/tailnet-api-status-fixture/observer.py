#!/usr/bin/env python3
"""Read public MCP through a normal host-launched agent, never private host IPC."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import queue
import subprocess
import threading
import time


class Rpc:
    def __init__(self, binary, evidence):
        self.events = queue.Queue(maxsize=64)
        self.next_id = 0
        self.evidence = evidence
        self.stderr = (evidence / "server.stderr").open("ab")
        self.process = subprocess.Popen(
            [str(binary), "--browser-mcp"], stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=self.stderr, env=os.environ.copy(),
        )
        threading.Thread(target=self.read, daemon=True).start()

    def read(self):
        while True:
            line = self.process.stdout.readline(1024 * 1024 + 1)
            try:
                value = json.loads(line) if line and len(line) <= 1024 * 1024 else None
                if value is not None and not isinstance(value, dict):
                    value = None
            except ValueError:
                value = None
            try:
                self.events.put(value, timeout=1)
            except queue.Full:
                return
            if value is None:
                return

    def send(self, message):
        self.process.stdin.write((json.dumps(message) + "\n").encode())
        self.process.stdin.flush()

    def request(self, method, params):
        self.next_id += 1
        request_id = self.next_id
        self.send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
        deadline = time.monotonic() + 20
        while True:
            message = self.events.get(timeout=max(.01, deadline - time.monotonic()))
            if message is None:
                raise RuntimeError("public MCP transport stopped")
            if message.get("id") == request_id:
                with (self.evidence / "rpc.jsonl").open("a") as log:
                    log.write(json.dumps({"at": time.time(), "method": method,
                                          "params": params, "response": message}) + "\n")
                return message
            if time.monotonic() >= deadline:
                raise RuntimeError("public MCP reply deadline exceeded")

    def close(self):
        if self.process.poll() is None:
            self.process.stdin.close()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.terminate()
                try:
                    self.process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    self.process.kill()
                    self.process.wait(timeout=5)
        self.stderr.close()


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
