#!/usr/bin/env python3
"""Exercise the public Horizon browser MCP contract against one live panel."""

from __future__ import annotations

import argparse
import base64
import json
import os
import re
import selectors
import subprocess
import sys
import time
from pathlib import Path
from typing import Any, Sequence


TOOL_NAMES = [
    "browser_act",
    "browser_audit",
    "browser_evaluate",
    "browser_handoff",
    "browser_list",
    "browser_navigate",
    "browser_network",
    "browser_panel",
    "browser_query",
    "browser_snapshot",
    "browser_wait",
]
PRIVATE_MARKERS = [
    "browser_ws",
    "automation_ws",
    "webSocketUrl",
    "manifest_path",
    "audit_path",
    "result_path",
    "devtools/browser",
    "devtools/page",
    ".horizon/runtime",
]
CAPABILITIES = [
    "navigate",
    "snapshot",
    "query",
    "click",
    "fill",
    "scroll",
    "reload",
    "back",
    "forward",
    "evaluate",
    "wait",
    "handoff",
    "audit",
]
NETWORK_CAPABILITIES = [
    "network_capture",
    "http_response_body_capture",
    "websocket_capture",
    "ndjson_export",
]


class McpClient:
    def __init__(self, command: Path, log_path: Path, timeout: float, actor: str) -> None:
        self.log = log_path.open("w", encoding="utf-8")
        environment = os.environ.copy()
        environment["HORIZON_BROWSER_ACTOR"] = actor
        environment["RUST_LOG"] = "off"
        self.process = subprocess.Popen(
            [str(command), "--browser-mcp"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=self.log,
            text=True,
            bufsize=1,
            env=environment,
        )
        if self.process.stdin is None or self.process.stdout is None:
            raise RuntimeError("could not open MCP stdio")
        self.timeout = timeout
        self.next_id = 1
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.process.stdout, selectors.EVENT_READ)

    def request(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        request_id = self.next_id
        self.next_id += 1
        message: dict[str, Any] = {"jsonrpc": "2.0", "id": request_id, "method": method}
        if params is not None:
            message["params"] = params
        assert self.process.stdin is not None
        self.process.stdin.write(json.dumps(message, separators=(",", ":")) + "\n")
        self.process.stdin.flush()

        deadline = time.monotonic() + self.timeout
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(f"MCP request timed out: {method}")
            if not self.selector.select(remaining):
                raise TimeoutError(f"MCP request timed out: {method}")
            assert self.process.stdout is not None
            line = self.process.stdout.readline()
            if not line:
                raise RuntimeError(f"MCP server closed while waiting for {method}")
            response = json.loads(line)
            if response.get("id") == request_id:
                return response

    def notify(self, method: str) -> None:
        assert self.process.stdin is not None
        self.process.stdin.write(json.dumps({"jsonrpc": "2.0", "method": method}) + "\n")
        self.process.stdin.flush()

    def call(
        self,
        name: str,
        arguments: dict[str, Any],
        *,
        expect_error: bool = False,
    ) -> tuple[dict[str, Any] | None, str | None]:
        response = self.request("tools/call", {"name": name, "arguments": arguments})
        if "error" in response:
            error = json.dumps(response["error"], sort_keys=True)
            if expect_error:
                return None, error
            raise AssertionError(f"{name} JSON-RPC error: {error}")
        result = response["result"]
        error = json.dumps(result, sort_keys=True)
        if expect_error:
            if not result.get("isError"):
                raise AssertionError(f"{name} unexpectedly succeeded: {result}")
            return None, error
        if result.get("isError"):
            raise AssertionError(f"{name} failed: {result}")
        structured = result.get("structuredContent")
        if not isinstance(structured, dict):
            raise AssertionError(f"{name} omitted structuredContent")
        assert_no_private_markers(
            json.dumps(structured, sort_keys=True, ensure_ascii=False),
            f"{name} result",
        )
        return structured, None

    def close(self) -> None:
        self.selector.close()
        if self.process.stdin is not None:
            self.process.stdin.close()
        status = self.process.wait(timeout=10)
        self.log.close()
        if status != 0:
            raise AssertionError(f"MCP server exited with status {status}")


def initialize(client: McpClient) -> list[dict[str, Any]]:
    response = client.request(
        "initialize",
        {
            "protocolVersion": "2026-07-28",
            "capabilities": {},
            "clientInfo": {"name": "horizon-browser-smoke", "version": "1"},
        },
    )
    result = response["result"]
    if result["protocolVersion"] != "2026-07-28":
        raise AssertionError(result)
    if result["serverInfo"]["name"] != "horizon-browser":
        raise AssertionError(result)
    if "sole agent control contract" not in result["instructions"]:
        raise AssertionError(result["instructions"])
    if "browser_network start before browser_navigate" not in result["instructions"]:
        raise AssertionError("MCP instructions do not teach the network capture workflow")
    client.notify("notifications/initialized")

    tools = client.request("tools/list")["result"]["tools"]
    if sorted(tool["name"] for tool in tools) != TOOL_NAMES:
        raise AssertionError(tools)
    encoded = json.dumps(tools, sort_keys=True)
    assert_no_private_markers(encoded, "tool schemas")
    act = next(tool for tool in tools if tool["name"] == "browser_act")
    count = act["inputSchema"]["properties"].get("count")
    if count is None:
        raise AssertionError("browser_act does not advertise click count")
    network = next(tool for tool in tools if tool["name"] == "browser_network")
    if "tail -f" not in network["description"] or "Start only" not in json.dumps(network["inputSchema"]):
        raise AssertionError("browser_network is not self-discovering")
    return tools


def wait_for_panel(client: McpClient, backend: str, timeout: float) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    last_panels: list[dict[str, Any]] = []
    while time.monotonic() < deadline:
        listed, _ = client.call("browser_list", {})
        assert listed is not None
        last_panels = listed["panels"]
        for panel in last_panels:
            if panel["backend"] == backend and panel["panel_id"]:
                return panel
        time.sleep(0.2)
    raise AssertionError(f"no ready {backend} panel appeared: {last_panels}")


def record_action(
    client: McpClient,
    name: str,
    arguments: dict[str, Any],
    action_ids: list[str],
) -> dict[str, Any]:
    result, _ = client.call(name, arguments)
    assert result is not None
    action_id = result.get("action_id")
    if isinstance(action_id, str):
        action_ids.append(action_id)
    return result


def wait_for_fingerprint(
    client: McpClient,
    panel_id: str,
    action_ids: list[str],
) -> dict[str, Any]:
    expression = "({fingerprint: window.__fingerprint ?? null, iframe: window.__iframeWebdriver ?? null})"
    deadline = time.monotonic() + 10
    value: Any = None
    while time.monotonic() < deadline:
        result = record_action(
            client,
            "browser_evaluate",
            {"panel_id": panel_id, "expression": expression},
            action_ids,
        )
        value = result["value"]
        if isinstance(value, dict) and isinstance(value.get("fingerprint"), dict) and value.get("iframe") is not None:
            return value["fingerprint"]
        time.sleep(0.1)
    raise AssertionError(f"fingerprint did not settle: {value}")


def verify_disclosure(fingerprint: dict[str, Any], backend: str, policy: str) -> None:
    observed = [
        fingerprint.get("earlyWebdriver"),
        fingerprint.get("currentWebdriver"),
        fingerprint.get("iframeWebdriver"),
    ]
    if policy == "browser_default":
        if observed != [True, True, True]:
            raise AssertionError(f"browser-default disclosure mismatch: {fingerprint}")
    elif backend in {"chromium", "firefox"} and observed != [False, False, False]:
        raise AssertionError(f"common-signal minimization mismatch: {fingerprint}")
    if policy == "minimize_common_signals" and backend == "chromium":
        if "HeadlessChrome" in str(fingerprint.get("userAgent")):
            raise AssertionError(f"headless token remained: {fingerprint}")


def failed_action_id(error: str) -> str:
    match = re.search(r"browser action ([^ ]+) failed", error)
    if match is None:
        raise AssertionError(f"error omitted action id: {error}")
    return match.group(1)


def verify_audit(
    client: McpClient,
    panel_id: str,
    action_ids: list[str],
    failed_ids: list[str],
    double_click_id: str,
) -> list[dict[str, Any]]:
    audit, _ = client.call("browser_audit", {"panel_id": panel_id, "limit": 500})
    assert audit is not None
    encoded = json.dumps(audit, sort_keys=True, ensure_ascii=False)
    for secret in [
        "MCP_USER_SECRET",
        "MCP_PASSWORD_SECRET",
        "MCP_URL_SECRET",
        "MCP_FRAGMENT_SECRET",
        "MCP_FILL_SECRET",
        "MCP_SELECTOR_SECRET",
        "MCP_SCRIPT_SECRET",
    ]:
        if secret in encoded:
            raise AssertionError(f"audit leaked {secret}")
    assert_no_private_markers(encoded, "audit")

    statuses: dict[str, set[str]] = {}
    double_entries: list[dict[str, Any]] = []
    for entry in audit["entries"]:
        statuses.setdefault(entry["action_id"], set()).add(entry["status"])
        if entry["action_id"] == double_click_id:
            double_entries.append(entry)
    for action_id in action_ids:
        expected = {"queued", "dispatched", "completed"}
        if not expected.issubset(statuses.get(action_id, set())):
            raise AssertionError((action_id, statuses.get(action_id)))
    for action_id in failed_ids:
        if "failed" not in statuses.get(action_id, set()):
            raise AssertionError((action_id, statuses.get(action_id)))
    if not double_entries or any(entry["action"].get("count") != 2 for entry in double_entries):
        raise AssertionError(double_entries)
    return audit["entries"]


def assert_no_private_markers(encoded: str, context: str) -> None:
    for marker in PRIVATE_MARKERS:
        if marker in encoded:
            raise AssertionError(f"{context} exposed private marker {marker}")


def exercise_network_capture(
    client: McpClient,
    args: argparse.Namespace,
    panel_id: str,
    action_ids: list[str],
    failed_ids: list[str],
) -> dict[str, Any]:
    if args.backend == "safari":
        _, error = client.call(
            "browser_network",
            {"panel_id": panel_id, "operation": "start"},
            expect_error=True,
        )
        assert error is not None and "unsupported_backend" in error
        failed_ids.append(failed_action_id(error))
        return {"supported": False}

    started = record_action(
        client,
        "browser_network",
        {
            "panel_id": panel_id,
            "operation": "start",
            "include_http": True,
            "include_http_bodies": True,
            "url_patterns": ["/websocket.html", "/market-stream"],
            "max_payload_bytes": 4096,
            "max_file_bytes": 16 * 1024 * 1024,
        },
        action_ids,
    )
    capture_path = Path(started["path"])
    if not capture_path.is_absolute() or not capture_path.is_file():
        raise AssertionError(f"capture path is not an explicit readable file: {capture_path}")
    if os.name != "nt" and capture_path.stat().st_mode & 0o777 != 0o600:
        raise AssertionError(f"capture file mode is not private: {capture_path}")
    record_action(
        client,
        "browser_navigate",
        {"panel_id": panel_id, "url": f"{args.base_url}/websocket.html"},
        action_ids,
    )
    record_action(
        client,
        "browser_wait",
        {
            "panel_id": panel_id,
            "selector": "#stream-status[data-state='closed']",
            "state": "visible",
            "timeout_millis": 30000,
        },
        action_ids,
    )

    deadline = time.monotonic() + 30
    status: dict[str, Any] | None = None
    while time.monotonic() < deadline:
        status = record_action(
            client,
            "browser_network",
            {"panel_id": panel_id, "operation": "status"},
            action_ids,
        )
        connections = status["known_connections"]
        if connections and connections[0]["state"] == "closed" and connections[0]["received_frames"] >= 4096:
            break
        time.sleep(0.1)
    else:
        raise AssertionError(f"network capture did not converge: {status}")

    stopped = record_action(
        client,
        "browser_network",
        {"panel_id": panel_id, "operation": "stop"},
        action_ids,
    )
    if stopped["active"]:
        raise AssertionError(stopped)
    if stopped["records_dropped"] or stopped["writer_failed"] or stopped["file_limit_reached"]:
        raise AssertionError(stopped)
    records = [json.loads(line) for line in capture_path.read_text(encoding="utf-8").splitlines()]
    if not records:
        raise AssertionError("network capture file is empty")
    sequences = [record["sequence"] for record in records]
    if sequences != sorted(sequences) or len(sequences) != len(set(sequences)):
        raise AssertionError("network capture sequence is not monotonic and unique")
    kinds = {record["kind"] for record in records}
    required = {
        "capture_started",
        "http_request",
        "websocket_created",
        "websocket_opened",
        "websocket_frame_sent",
        "websocket_frame_received",
        "websocket_closed",
        "capture_stopped",
    }
    if not required.issubset(kinds):
        raise AssertionError({"missing": sorted(required - kinds), "kinds": sorted(kinds)})
    received = [record for record in records if record["kind"] == "websocket_frame_received"]
    if len(received) != 4096:
        raise AssertionError(f"expected 4096 received frames, got {len(received)}")
    if json.loads(received[-1]["payload"])["sequence"] != 4095:
        raise AssertionError("network capture omitted the final high-rate frame")
    bodies = [record for record in records if record["kind"] == "http_response_body"]
    if not bodies:
        raise AssertionError("network capture omitted the fixture HTTP response body")
    fixture_bodies = [record for record in bodies if "/websocket.html" in record.get("url", "")]
    if not fixture_bodies:
        raise AssertionError("network capture did not associate the fixture body with its URL")
    fixture_body = fixture_bodies[-1]
    if fixture_body.get("error") or fixture_body.get("truncated"):
        raise AssertionError(f"fixture HTTP response body was incomplete: {fixture_body}")
    payload = fixture_body.get("payload", "")
    if fixture_body.get("payload_encoding") == "base64":
        payload = base64.b64decode(payload, validate=True).decode("utf-8")
    if "WebSocket capture fixture" not in payload:
        raise AssertionError("fixture HTTP response body payload was incorrect")
    urls = [record.get("url", "") for record in records]
    if any("MCP_NETWORK_URL_SECRET" in url for url in urls):
        raise AssertionError("network capture leaked URL query data")
    return {
        "capture_id": stopped["capture_id"],
        "path": str(capture_path),
        "records": len(records),
        "response_bodies": len(bodies),
        "received_frames": len(received),
        "supported": True,
        "transport": stopped["transport"],
    }


def exercise_capture_retention(
    client: McpClient,
    panel_id: str,
    capture_path: Path,
    action_ids: list[str],
) -> dict[str, Any]:
    directory = capture_path.parent
    seeds = [directory / f"retention-seed-{index:02d}.ndjson" for index in range(65)]
    for seed in seeds:
        seed.write_bytes(b"x")
    oversized = directory / "retention-oversized.ndjson"
    with oversized.open("wb") as output:
        output.truncate(1024 * 1024 * 1024)
    expired = directory / "retention-expired.ndjson"
    expired.write_bytes(b"x")
    os.utime(expired, (1, 1))
    # Keep the real completed export newer than the synthetic pressure files
    # so the oldest-first policy must preserve useful recent agent output.
    future = time.time() + 60
    os.utime(capture_path, (future, future))

    started = record_action(
        client,
        "browser_network",
        {
            "panel_id": panel_id,
            "operation": "start",
            "url_patterns": ["/retention-probe"],
            "max_payload_bytes": 1024,
            "max_file_bytes": 1024 * 1024,
        },
        action_ids,
    )
    retained = sorted(directory.glob("*.ndjson"))
    if expired.exists():
        raise AssertionError("capture retention kept an expired export")
    if oversized.exists():
        raise AssertionError("capture retention did not reserve the aggregate byte budget")
    if len(retained) > 64:
        raise AssertionError(f"capture retention kept {len(retained)} files, expected at most 64")
    if sum(path.stat().st_size for path in retained) > 1024 * 1024 * 1024:
        raise AssertionError("capture retention exceeded the aggregate byte limit")
    if not capture_path.exists():
        raise AssertionError("capture retention removed the newest completed export")

    stopped = record_action(
        client,
        "browser_network",
        {"panel_id": panel_id, "operation": "stop"},
        action_ids,
    )
    for seed in [*seeds, oversized, expired]:
        seed.unlink(missing_ok=True)
    return {
        "max_files": 64,
        "max_total_bytes": 1024 * 1024 * 1024,
        "path": started["path"],
        "retained_during_probe": len(retained),
        "stopped": not stopped["active"],
    }


def exercise(client: McpClient, args: argparse.Namespace) -> dict[str, Any]:
    initialize(client)
    panel = wait_for_panel(client, args.backend, args.panel_timeout)
    panel_id = panel["panel_id"]
    expected_protocols = {
        "chromium": {"cdp"},
        "firefox": {"web_driver_bidi"},
        "safari": {"web_driver", "web_driver_bidi"},
    }
    if panel["protocol"] not in expected_protocols[args.backend]:
        raise AssertionError(panel)
    detail, _ = client.call("browser_panel", {"panel_id": panel_id})
    assert detail is not None
    expected_capabilities = CAPABILITIES + ([] if args.backend == "safari" else NETWORK_CAPABILITIES)
    if detail["capabilities"] != expected_capabilities:
        raise AssertionError(detail)
    network_capability = detail["network_capture"]
    if network_capability["supported"] != (args.backend != "safari"):
        raise AssertionError(network_capability)
    expected_body_transport = {
        "chromium": "cdp",
        "firefox": "webdriver_bidi",
        "safari": None,
    }
    if network_capability["http_response_body_transport"] != expected_body_transport[args.backend]:
        raise AssertionError(network_capability)
    if args.backend == "firefox" and not network_capability["page_instrumentation"]:
        raise AssertionError(network_capability)
    if args.backend == "chromium" and network_capability["page_instrumentation"]:
        raise AssertionError(network_capability)

    action_ids: list[str] = []
    failed_ids: list[str] = []
    network_capture = exercise_network_capture(client, args, panel_id, action_ids, failed_ids)
    secret_origin = args.base_url.replace(
        "://",
        "://MCP_USER_SECRET:MCP_PASSWORD_SECRET@",
        1,
    )
    secret_url = f"{secret_origin}/index.html?token=MCP_URL_SECRET#MCP_FRAGMENT_SECRET"
    record_action(client, "browser_navigate", {"panel_id": panel_id, "url": secret_url}, action_ids)
    record_action(
        client,
        "browser_wait",
        {"panel_id": panel_id, "selector": "#smoke-input", "state": "visible"},
        action_ids,
    )

    click = record_action(
        client,
        "browser_query",
        {"panel_id": panel_id, "selector": "#click-target", "max_results": 1},
        action_ids,
    )
    click_ref = click["nodes"][0]["ref"]
    record_action(
        client,
        "browser_act",
        {"panel_id": panel_id, "action": "click", "ref": click_ref},
        action_ids,
    )

    double = record_action(
        client,
        "browser_query",
        {"panel_id": panel_id, "selector": "#double-target", "max_results": 1},
        action_ids,
    )
    double_result = record_action(
        client,
        "browser_act",
        {"panel_id": panel_id, "action": "click", "ref": double["nodes"][0]["ref"], "count": 2},
        action_ids,
    )
    double_click_id = double_result["action_id"]
    status = record_action(
        client,
        "browser_evaluate",
        {"panel_id": panel_id, "expression": "document.querySelector('#status').textContent"},
        action_ids,
    )["value"]
    if "clicks=1" not in status or "double=1" not in status or "trusted=true" not in status:
        raise AssertionError(status)

    field = record_action(
        client,
        "browser_query",
        {"panel_id": panel_id, "selector": "#smoke-input", "max_results": 1},
        action_ids,
    )
    field_ref = field["nodes"][0]["ref"]
    marker = f"MCP_FILL_SECRET_{args.backend}_æøå_✓"
    record_action(
        client,
        "browser_act",
        {"panel_id": panel_id, "action": "fill", "ref": field_ref, "value": marker},
        action_ids,
    )
    filled = record_action(
        client,
        "browser_evaluate",
        {"panel_id": panel_id, "expression": "document.querySelector('#smoke-input').value"},
        action_ids,
    )["value"]
    if filled != marker:
        raise AssertionError(filled)

    record_action(client, "browser_snapshot", {"panel_id": panel_id, "max_nodes": 100}, action_ids)
    _, stale_error = client.call(
        "browser_act",
        {"panel_id": panel_id, "action": "fill", "ref": field_ref, "value": "stale"},
        expect_error=True,
    )
    assert stale_error is not None and "stale_reference" in stale_error
    failed_ids.append(failed_action_id(stale_error))
    _, selector_error = client.call(
        "browser_query",
        {"panel_id": panel_id, "selector": "["},
        expect_error=True,
    )
    assert selector_error is not None
    failed_ids.append(failed_action_id(selector_error))

    record_action(
        client,
        "browser_query",
        {"panel_id": panel_id, "selector": "#smoke-input[data-secret='MCP_SELECTOR_SECRET']"},
        action_ids,
    )
    script_value = record_action(
        client,
        "browser_evaluate",
        {"panel_id": panel_id, "expression": "'MCP_SCRIPT_SECRET_' + document.title"},
        action_ids,
    )["value"]
    if not script_value.startswith("MCP_SCRIPT_SECRET_"):
        raise AssertionError(script_value)

    fingerprint = wait_for_fingerprint(client, panel_id, action_ids)
    verify_disclosure(fingerprint, args.backend, args.automation_disclosure)
    record_action(
        client,
        "browser_act",
        {"panel_id": panel_id, "action": "scroll", "delta_x": 0, "delta_y": 900},
        action_ids,
    )
    scroll_y = record_action(
        client,
        "browser_evaluate",
        {"panel_id": panel_id, "expression": "scrollY"},
        action_ids,
    )["value"]
    if not isinstance(scroll_y, (int, float)) or scroll_y <= 0:
        raise AssertionError(scroll_y)

    record_action(
        client,
        "browser_navigate",
        {"panel_id": panel_id, "url": f"{args.base_url}/next.html"},
        action_ids,
    )
    record_action(
        client,
        "browser_wait",
        {"panel_id": panel_id, "selector": "#next-marker", "state": "visible"},
        action_ids,
    )
    record_action(client, "browser_act", {"panel_id": panel_id, "action": "back"}, action_ids)
    record_action(
        client,
        "browser_wait",
        {"panel_id": panel_id, "selector": "#smoke-input", "state": "visible"},
        action_ids,
    )
    record_action(client, "browser_act", {"panel_id": panel_id, "action": "forward"}, action_ids)
    record_action(
        client,
        "browser_wait",
        {"panel_id": panel_id, "selector": "#next-marker", "state": "visible"},
        action_ids,
    )
    record_action(client, "browser_act", {"panel_id": panel_id, "action": "reload"}, action_ids)
    final_snapshot = record_action(client, "browser_snapshot", {"panel_id": panel_id}, action_ids)
    if final_snapshot["title"] != "Horizon Browser Smoke - Navigation Complete":
        raise AssertionError(final_snapshot)

    retained_title = final_snapshot["title"]
    if args.backend == "safari":
        record_action(
            client,
            "browser_navigate",
            {
                "panel_id": panel_id,
                "url": f"{args.base_url}/slow-navigation.html",
                "timeout_millis": 60000,
            },
            action_ids,
        )
        record_action(
            client,
            "browser_wait",
            {
                "panel_id": panel_id,
                "selector": "#slow-marker",
                "state": "visible",
                "timeout_millis": 60000,
            },
            action_ids,
        )
        slow_snapshot = record_action(client, "browser_snapshot", {"panel_id": panel_id}, action_ids)
        retained_title = slow_snapshot["title"]
        if retained_title != "Horizon Browser Smoke - Slow Navigation":
            raise AssertionError(slow_snapshot)

    _, navigation_error = client.call(
        "browser_navigate",
        {"panel_id": panel_id, "url": "http://[::1"},
        expect_error=True,
    )
    if navigation_error is None or not any(
        code in navigation_error for code in ("navigation_failed", "protocol_error")
    ):
        raise AssertionError(f"immediate navigation failure was not typed: {navigation_error}")
    failed_ids.append(failed_action_id(navigation_error))
    retained_snapshot = record_action(client, "browser_snapshot", {"panel_id": panel_id}, action_ids)
    if retained_snapshot["title"] != retained_title:
        raise AssertionError(retained_snapshot)

    if network_capture["supported"]:
        network_capture["retention"] = exercise_capture_retention(
            client,
            panel_id,
            Path(network_capture["path"]),
            action_ids,
        )
        active_close = record_action(
            client,
            "browser_network",
            {
                "panel_id": panel_id,
                "operation": "start",
                "url_patterns": ["/active-close-fixture"],
                "max_payload_bytes": 1024,
                "max_file_bytes": 1024 * 1024,
            },
            action_ids,
        )
        network_capture["active_close_path"] = active_close["path"]

    audit_entries = len(verify_audit(client, panel_id, action_ids, failed_ids, double_click_id))
    handoff_request = None
    if args.handoff:
        handoff, _ = client.call(
            "browser_handoff",
            {"panel_id": panel_id, "reason": f"Complete the visible {args.backend} hand-back gate"},
        )
        assert handoff is not None and handoff["handoff_pending"] is True
        handoff_request = handoff["request_id"]
        _, blocked = client.call(
            "browser_act",
            {"panel_id": panel_id, "action": "reload"},
            expect_error=True,
        )
        assert blocked is not None and "would block" in blocked.lower()
        print(
            json.dumps(
                {
                    "handoff_pending": True,
                    "instruction": "Click Done - hand back to agent in the exact smoke window",
                    "panel_id": panel_id,
                    "request_id": handoff_request,
                },
                sort_keys=True,
            ),
            flush=True,
        )
        deadline = time.monotonic() + args.handoff_timeout
        while time.monotonic() < deadline:
            current, _ = client.call("browser_panel", {"panel_id": panel_id})
            assert current is not None
            if not current["handoff_pending"]:
                record_action(client, "browser_snapshot", {"panel_id": panel_id}, action_ids)
                handoff_entries = verify_audit(
                    client,
                    panel_id,
                    action_ids,
                    failed_ids,
                    double_click_id,
                )
                requested_indexes = [
                    index
                    for index, entry in enumerate(handoff_entries)
                    if entry["action_id"] == handoff_request
                    and entry["status"] == "dispatched"
                    and entry["action"].get("type") == "handoff_requested"
                ]
                if len(requested_indexes) != 1:
                    raise AssertionError("handoff request was not audited exactly once")
                done_indexes = [
                    index
                    for index, entry in enumerate(handoff_entries)
                    if index > requested_indexes[0]
                    and entry["status"] == "dispatched"
                    and entry["actor"].get("type") == "user"
                    and entry["action"].get("type") == "handoff_done"
                ]
                if len(done_indexes) != 1:
                    raise AssertionError("handoff completion was not audited exactly once")
                if not any(
                    index > requested_indexes[0]
                    and index < done_indexes[0]
                    and entry["status"] == "rejected"
                    and entry["actor"].get("type") == "agent"
                    and entry["action"].get("type") == "reload"
                    for index, entry in enumerate(handoff_entries)
                ):
                    raise AssertionError("the MCP action blocked during handoff was not audited")
                audit_entries = len(handoff_entries)
                break
            time.sleep(0.25)
        else:
            raise AssertionError("handoff remained pending")

    return {
        "audit_entries": audit_entries,
        "backend": args.backend,
        "completed_actions": len(action_ids),
        "double_click": {"count": 2, "trusted": True},
        "handoff_request_id": handoff_request,
        "network_capture": network_capture,
        "panel_id": panel_id,
        "protocol": panel["protocol"],
        "scroll_y": scroll_y,
    }


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--horizon", type=Path, required=True, help="exact Horizon binary under test")
    parser.add_argument("--backend", choices=["chromium", "firefox", "safari"], required=True)
    parser.add_argument("--base-url", required=True, help="base URL printed by fixture_server.py")
    parser.add_argument(
        "--automation-disclosure",
        choices=["minimize_common_signals", "browser_default"],
        default="minimize_common_signals",
    )
    parser.add_argument("--log", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=60, help="individual MCP request timeout")
    parser.add_argument("--panel-timeout", type=float, default=90)
    parser.add_argument("--handoff", action="store_true", help="request handoff and wait for visible user hand-back")
    parser.add_argument("--handoff-timeout", type=float, default=300)
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    command = args.horizon.resolve()
    if not command.is_file():
        raise SystemExit(f"Horizon binary does not exist: {command}")
    args.log.parent.mkdir(parents=True, exist_ok=True)
    actor = f"browser-smoke:{args.backend}:{os.getpid()}"
    client = McpClient(command, args.log, args.timeout, actor)
    try:
        result = exercise(client, args)
        print(json.dumps(result, ensure_ascii=False, sort_keys=True), flush=True)
    finally:
        client.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
