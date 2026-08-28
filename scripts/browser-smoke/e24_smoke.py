#!/usr/bin/env python3
"""Run a live E24 market-data correctness and timing gate through Horizon MCP."""

from __future__ import annotations

import argparse
import base64
import json
import os
import platform
import re
import subprocess
import sys
import time
import traceback
from decimal import Decimal, InvalidOperation
from pathlib import Path
from typing import Any, Sequence
from zoneinfo import ZoneInfo

sys.dont_write_bytecode = True
import mcp_gate
import run as browser_smoke


E24_URL = "https://e24.no/bors"
DOM_QUOTES_EXPRESSION = r"""(() => {
  const clean = value => String(value ?? '').replace(/[\u00a0\u202f]/g, ' ').replace(/\s+/g, ' ').trim();
  const valueOf = element => clean(element?.textContent || element?.getAttribute?.('aria-label') || element?.title);
  const quotes = Array.from(document.querySelectorAll('table.millistream-list-table tr')).map((row, index) => {
    const link = row.querySelector('a[href]');
    const price = row.querySelector('.millistream-lastprice,[class*="lastprice"]');
    const change = row.querySelector('.millistream-diff1dprc,[class*="diff1dprc"]');
    const symbol = row.querySelector('.millistream-symbol,.millistream-shortname,.millistream-ticker,[class*="shortname"],[class*="ticker"]');
    const name = row.querySelector('.millistream-name,[class*="instrument-name"]');
    const identifiers = [link?.href || '', valueOf(link), valueOf(symbol), valueOf(name)];
    return { index, text: valueOf(row), href: link?.href || '', linkText: valueOf(link),
      symbol: valueOf(symbol), name: valueOf(name), price: valueOf(price),
      change: valueOf(change), identifiers };
  }).filter(quote => /\d/.test(quote.price) && (quote.href || quote.linkText || quote.symbol || quote.name));
  return { observedAtMillis: Date.now(), title: document.title, url: location.href, quotes: quotes.slice(0, 200) };
})()"""
def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--backend", choices=["chromium", "firefox"], default="firefox")
    parser.add_argument("--horizon", type=Path, default=Path("target/debug/horizon"))
    parser.add_argument("--root", type=Path)
    parser.add_argument("--url", default=E24_URL)
    parser.add_argument("--observation-seconds", type=float, default=60.0)
    parser.add_argument("--feed-timeout-seconds", type=float, default=90.0)
    parser.add_argument("--minimum-matches", type=int, default=3)
    parser.add_argument("--allow-closed-market", action="store_true")
    parser.add_argument("--allow-dirty", action="store_true")
    parser.add_argument("--keep-open", action="store_true")
    parser.add_argument("--chromium-command", type=Path)
    parser.add_argument("--firefox-command", type=Path)
    parser.add_argument("--geckodriver-command", type=Path)
    parser.add_argument("--safaridriver-command", type=Path)
    parser.add_argument(
        "--automation-disclosure",
        choices=["minimize_common_signals", "browser_default"],
        default="minimize_common_signals",
    )
    return parser.parse_args(argv)

def market_is_open(now: Any | None = None) -> bool:
    local = now or __import__("datetime").datetime.now(ZoneInfo("Europe/Oslo"))
    minute = local.hour * 60 + local.minute
    return local.weekday() < 5 and 9 * 60 <= minute <= 16 * 60 + 20

def write_config(args: argparse.Namespace, root: Path) -> Path:
    config = {
        "version": 10,
        "window": {"width": 1400, "height": 900},
        "browser": browser_smoke.browser_config(args, root),
        "workspaces": [{
            "name": "E24 Market Data Smoke",
            "position": [30, 30],
            "terminals": [{
                "name": f"E24 {args.backend.title()}", "kind": "browser", "command": "about:blank",
                "position": [30, 30], "size": [1080, 720],
            }],
        }],
    }
    path = root / "e24.json"
    path.write_text(json.dumps(config, indent=2) + "\n", encoding="utf-8")
    return path

def mcp_client(command: Path, log: Path, environment: dict[str, str], backend: str) -> mcp_gate.McpClient:
    original = os.environ.copy()
    os.environ.clear()
    os.environ.update(environment)
    try:
        return mcp_gate.McpClient(command, log, 60, f"e24-smoke:{backend}:{os.getpid()}")
    finally:
        os.environ.clear()
        os.environ.update(original)

def evaluate_quotes(
    client: mcp_gate.McpClient,
    panel_id: str,
    action_ids: list[str],
) -> dict[str, Any]:
    result = mcp_gate.record_action(
        client,
        "browser_evaluate",
        {"panel_id": panel_id, "expression": DOM_QUOTES_EXPRESSION, "timeout_millis": 30_000},
        action_ids,
    )["value"]
    if not isinstance(result, dict) or not isinstance(result.get("quotes"), list):
        raise AssertionError(f"E24 DOM quote extraction returned an unexpected value: {result}")
    return result

def wait_for_payload(path: Path, timeout: float, after_sequence: int = 0) -> dict[str, Any] | None:
    deadline = time.monotonic() + timeout
    with path.open("r", encoding="utf-8") as stream:
        while time.monotonic() < deadline:
            line = stream.readline()
            if not line:
                time.sleep(0.025)
                continue
            record = json.loads(line)
            if record.get("kind") == "http_response_body" and int(record.get("sequence", 0)) > after_sequence:
                return record
    return None

def last_capture_sequence(path: Path) -> int:
    sequence = 0
    for line in path.read_text(encoding="utf-8").splitlines():
        try:
            sequence = max(sequence, int(json.loads(line).get("sequence", 0)))
        except (json.JSONDecodeError, TypeError, ValueError):
            continue
    return sequence

def decimal_value(value: str) -> Decimal | None:
    match = re.search(r"[-+]?\d[\d\s\u00a0\u202f.]*(?:,\d+)?|[-+]?\d+(?:\.\d+)?", value)
    if match is None:
        return None
    token = re.sub(r"[\s\u00a0\u202f]", "", match.group(0))
    if "," in token:
        token = token.replace(".", "").replace(",", ".")
    try:
        return Decimal(token)
    except InvalidOperation:
        return None

def payload_text(record: dict[str, Any]) -> str:
    payload = record.get("payload")
    if not isinstance(payload, str):
        return ""
    if record.get("payload_encoding") == "base64":
        try:
            raw = base64.b64decode(payload, validate=True)
        except ValueError:
            return ""
        return raw.decode("utf-8", errors="replace").replace("\x00", " ")
    return payload

def json_decimal(value: Any) -> Decimal | None:
    try:
        return Decimal(str(value))
    except (InvalidOperation, TypeError, ValueError):
        return None

def feed_items(record: dict[str, Any]) -> list[dict[str, Any]]:
    text = payload_text(record).strip()
    match = re.fullmatch(r"[A-Za-z_$][\w$]*(?:\.[A-Za-z_$][\w$]*)*\s*\((.*)\)\s*;?", text, re.DOTALL)
    if match is None:
        return []
    try:
        value = json.loads(match.group(1))
    except json.JSONDecodeError:
        return []
    if isinstance(value, list):
        return [item for item in value if isinstance(item, dict) and "lastprice" in item]
    if not isinstance(value, dict):
        return []
    basics = {
        str(item.get("insref")): item
        for item in value.get("basicdata", [])
        if isinstance(item, dict) and item.get("insref") is not None
    }
    return [
        {**basics.get(str(quote.get("insref")), {}), **quote}
        for quote in value.get("quotes", [])
        if isinstance(quote, dict) and "lastprice" in quote
    ]

def match_quotes(dom: dict[str, Any], records: list[dict[str, Any]]) -> list[dict[str, Any]]:
    observed_at = int(dom["observedAtMillis"])
    payloads: list[tuple[dict[str, Any], dict[str, Any]]] = []
    for record in records:
        timestamp = record.get("timestamp_millis")
        if record.get("kind") != "http_response_body" or not isinstance(timestamp, int):
            continue
        if not observed_at - 15_000 <= timestamp <= observed_at + 1_000:
            continue
        payloads.extend((record, item) for item in feed_items(record))
    matches: list[dict[str, Any]] = []
    for quote in dom["quotes"]:
        price_text = quote.get("price")
        price = decimal_value(price_text) if isinstance(price_text, str) else None
        href = quote.get("href", "")
        instrument = re.search(r"/aksjer/(\d+)/", href) if isinstance(href, str) else None
        if price is None or instrument is None:
            continue
        change_text = quote.get("change")
        change = decimal_value(change_text) if isinstance(change_text, str) else None
        for record, item in reversed(payloads):
            feed_price = json_decimal(item.get("lastprice"))
            feed_change = json_decimal(item.get("diff1dprc"))
            if str(item.get("insref")) == instrument.group(1) and price == feed_price and change == feed_change:
                matches.append(
                    {
                        "change": str(feed_change),
                        "connection_id": record.get("connection_id"),
                        "insref": instrument.group(1),
                        "name": item.get("name") or quote.get("name") or quote.get("linkText"),
                        "price": str(price),
                        "response_sequence": record.get("sequence"),
                        "response_timestamp_millis": record.get("timestamp_millis"),
                        "symbol": item.get("symbol") or quote.get("symbol", ""),
                    }
                )
                break
    return matches

def summarize_market(records: list[dict[str, Any]], refresh_boundary: int) -> dict[str, Any]:
    initial: dict[str, tuple[int, dict[str, Any], Decimal, Decimal | None]] = {}
    final: dict[str, tuple[int, dict[str, Any], Decimal, Decimal | None]] = {}
    observations = 0
    timestamps: list[int] = []
    for record in records:
        timestamp = record.get("timestamp_millis")
        if record.get("kind") != "http_response_body" or not isinstance(timestamp, int):
            continue
        for item in feed_items(record):
            price = json_decimal(item.get("lastprice"))
            key = str(item.get("insref", ""))
            if not key or price is None or item.get("instrumenttype") != 4:
                continue
            target = final if int(record.get("sequence", 0)) > refresh_boundary else initial
            target[key] = (timestamp, item, price, json_decimal(item.get("diff1dprc")))
            observations += 1
            timestamps.append(timestamp)
    movers = sorted((value for value in final.values() if value[3] is not None), key=lambda value: value[3])
    changed = []
    for key in initial.keys() & final.keys():
        first, last = initial[key], final[key]
        if first[2] == last[2]:
            continue
        delta = last[2] - first[2]
        changed.append({
            "symbol": last[1].get("symbol", ""), "name": last[1].get("name", ""),
            "from": str(first[2]), "to": str(last[2]), "delta": str(delta),
            "delta_percent": str((delta / first[2] * 100).quantize(Decimal("0.0001"))) if first[2] else None,
        })
    compact = lambda value: {
        "symbol": value[1].get("symbol", ""), "name": value[1].get("name", ""),
        "price": str(value[2]), "day_change_percent": str(value[3]),
    }
    return {
        "changed_instruments": sorted(changed, key=lambda value: abs(Decimal(value["delta_percent"] or "0")), reverse=True)[:10],
        "feed_span_millis": max(timestamps) - min(timestamps) if timestamps else 0,
        "final_stocks": len(final),
        "initial_stocks": len(initial),
        "stock_observations": observations,
        "top_gainers": [compact(value) for value in reversed(movers[-5:])],
        "top_losers": [compact(value) for value in movers[:5]],
        "unique_stocks": len(initial.keys() | final.keys()),
    }

def verify_jq(path: Path) -> None:
    expression = 'length > 0 and .[0].kind == "capture_started" and .[-1].kind == "capture_stopped"'
    result = subprocess.run(
        ["jq", "-e", "-s", expression, str(path)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        raise AssertionError(f"jq could not validate the returned NDJSON path: {result.stderr.strip()}")


def verify_audit(client: mcp_gate.McpClient, panel_id: str, action_ids: list[str]) -> dict[str, Any]:
    audit, _ = client.call("browser_audit", {"panel_id": panel_id, "limit": 500})
    assert audit is not None
    encoded = json.dumps(audit, ensure_ascii=False, sort_keys=True)
    mcp_gate.assert_no_private_markers(encoded, "E24 audit")
    statuses: dict[str, set[str]] = {}
    for entry in audit["entries"]:
        statuses.setdefault(entry["action_id"], set()).add(entry["status"])
    missing = {
        action_id: sorted({"queued", "dispatched", "completed"} - statuses.get(action_id, set()))
        for action_id in action_ids
        if not {"queued", "dispatched", "completed"}.issubset(statuses.get(action_id, set()))
    }
    return {"entries": len(audit["entries"]), "missing_terminal_states": missing}

def exercise(client: mcp_gate.McpClient, args: argparse.Namespace, run_started: float) -> dict[str, Any]:
    mcp_gate.initialize(client)
    action_ids: list[str] = []
    panel = mcp_gate.wait_for_panel(client, args.backend, 90)
    panel_ready = time.monotonic()
    panel_id = panel["panel_id"]
    detail, _ = client.call("browser_panel", {"panel_id": panel_id})
    assert detail is not None
    if not detail["network_capture"]["supported"]:
        raise AssertionError(f"network capture is unavailable: {detail['network_capture']}")
    if detail["network_capture"].get("http_response_body_transport") not in {"cdp", "webdriver_bidi"}:
        raise AssertionError(f"native HTTP response bodies are unavailable: {detail['network_capture']}")
    started_at = time.monotonic()
    started = mcp_gate.record_action(
        client,
        "browser_network",
        {
            "panel_id": panel_id,
            "operation": "start",
            "include_http": True,
            "include_http_bodies": True,
            "include_websocket": False,
            "url_patterns": ["mws.fcgi"],
            "max_payload_bytes": 1024 * 1024,
            "max_file_bytes": 256 * 1024 * 1024,
        },
        action_ids,
    )
    capture_path = Path(started["path"])
    stopped: dict[str, Any] | None = None
    try:
        navigation_started = time.monotonic()
        mcp_gate.record_action(
            client,
            "browser_navigate",
            {"panel_id": panel_id, "url": args.url, "timeout_millis": 60_000},
            action_ids,
        )
        navigation_finished = time.monotonic()
        mcp_gate.record_action(
            client,
            "browser_wait",
            {
                "panel_id": panel_id,
                "selector": "table.millistream-list-table",
                "state": "present",
                "timeout_millis": 60_000,
            },
            action_ids,
        )
        dom_ready = time.monotonic()
        first_payload = wait_for_payload(capture_path, args.feed_timeout_seconds)
        first_payload_seen = time.monotonic()
        dom_start = evaluate_quotes(client, panel_id, action_ids)
        refreshed_payload: dict[str, Any] | None = None
        refresh_boundary = 0
        refresh_started: float | None = None
        refresh_finished: float | None = None
        refresh_payload_seen: float | None = None
        if first_payload is not None:
            deadline = time.monotonic() + args.observation_seconds
            while time.monotonic() < deadline:
                remaining = deadline - time.monotonic()
                print(
                    json.dumps({"capture_active": True, "remaining_seconds": round(max(0.0, remaining), 1)}),
                    flush=True,
                )
                time.sleep(min(5.0, max(0.05, remaining)))
            refresh_boundary = last_capture_sequence(capture_path)
            refresh_started = time.monotonic()
            mcp_gate.record_action(
                client,
                "browser_act",
                {"panel_id": panel_id, "action": "reload", "timeout_millis": 60_000},
                action_ids,
            )
            refresh_finished = time.monotonic()
            refreshed_payload = wait_for_payload(
                capture_path,
                args.feed_timeout_seconds,
                refresh_boundary,
            )
            refresh_payload_seen = time.monotonic()
            mcp_gate.record_action(
                client,
                "browser_wait",
                {
                    "panel_id": panel_id,
                    "selector": "table.millistream-list-table",
                    "state": "present",
                    "timeout_millis": 60_000,
                },
                action_ids,
            )
        dom_final_started = time.monotonic()
        dom_final = evaluate_quotes(client, panel_id, action_ids)
        dom_final_finished = time.monotonic()
        stopped = mcp_gate.record_action(
            client,
            "browser_network",
            {"panel_id": panel_id, "operation": "stop", "timeout_millis": 60_000},
            action_ids,
        )
    finally:
        if stopped is None:
            stopped, _ = client.call(
                "browser_network",
                {"panel_id": panel_id, "operation": "stop", "timeout_millis": 60_000},
            )
    assert stopped is not None
    stop_finished = time.monotonic()
    verify_jq(capture_path)
    records = [json.loads(line) for line in capture_path.read_text(encoding="utf-8").splitlines()]
    decode_started = time.monotonic()
    received = [record for record in records if record.get("kind") == "http_response_body"]
    initial_matches = match_quotes(dom_start, records)
    final_matches = match_quotes(dom_final, records)
    matches = initial_matches + final_matches
    unique_matches = {str((match["insref"], match["price"], match["change"])): match for match in matches}
    matches = list(unique_matches.values())
    market_summary = summarize_market(records, refresh_boundary)
    decode_finished = time.monotonic()
    audit = verify_audit(client, panel_id, action_ids)
    health_failures = [
        key
        for key in ["records_dropped", "payloads_truncated", "file_limit_reached", "writer_failed"]
        if stopped.get(key) not in (0, False)
    ]
    failures = []
    if first_payload is None:
        failures.append("no E24 market-data response body was captured")
    if refreshed_payload is None:
        failures.append("no E24 market-data response body was captured after the one-minute reload")
    if len(initial_matches) < args.minimum_matches:
        failures.append(
            f"only {len(initial_matches)} initial DOM prices matched captured market-data responses; need {args.minimum_matches}"
        )
    if len(final_matches) < args.minimum_matches:
        failures.append(
            f"only {len(final_matches)} refreshed DOM prices matched captured market-data responses; need {args.minimum_matches}"
        )
    if any(record.get("error") for record in received):
        failures.append("one or more E24 response bodies could not be retrieved")
    if health_failures:
        failures.append(f"capture health failed: {', '.join(health_failures)}")
    if audit["missing_terminal_states"]:
        failures.append("one or more MCP actions lack complete audit states")
    first_timestamp = first_payload.get("timestamp_millis") if first_payload else None
    refreshed_timestamp = refreshed_payload.get("timestamp_millis") if refreshed_payload else None
    capture_start_timestamp = records[0].get("timestamp_millis") if records else None
    return {
        "audit": audit,
        "backend": args.backend,
        "capture": {
            "bytes_written": stopped["bytes_written"],
            "connections": stopped["known_connections"],
            "path": str(capture_path),
            "response_body_bytes": sum(record.get("payload_bytes", 0) or 0 for record in received),
            "response_bodies": len(received),
            "records": len(records),
            "transport": stopped["transport"],
        },
        "dom": {
            "final_observed_at_millis": dom_final["observedAtMillis"],
            "final_rows": len(dom_final["quotes"]),
            "initial_observed_at_millis": dom_start["observedAtMillis"],
            "initial_rows": len(dom_start["quotes"]),
        },
        "failures": failures,
        "git_head": browser_smoke.git_head(Path(__file__).resolve().parents[2]),
        "market_summary": market_summary,
        "passed": not failures,
        "timings_ms": {
            "capture_start_to_first_payload": (
                first_timestamp - capture_start_timestamp
                if isinstance(first_timestamp, int) and isinstance(capture_start_timestamp, int)
                else None
            ),
            "decode_and_match": round((decode_finished - decode_started) * 1000, 3),
            "dom_final_query": round((dom_final_finished - dom_final_started) * 1000, 3),
            "horizon_launch_to_panel_ready": round((panel_ready - run_started) * 1000, 3),
            "navigation": round((navigation_finished - navigation_started) * 1000, 3),
            "panel_ready_to_capture_start": round((started_at - panel_ready) * 1000, 3),
            "feed_wait": round((first_payload_seen - dom_ready) * 1000, 3),
            "refresh": round((refresh_finished - refresh_started) * 1000, 3)
            if refresh_started is not None and refresh_finished is not None
            else None,
            "refresh_feed_wait": round((refresh_payload_seen - refresh_finished) * 1000, 3)
            if refresh_payload_seen is not None and refresh_finished is not None
            else None,
            "snapshot_interval": refreshed_timestamp - first_timestamp
            if isinstance(refreshed_timestamp, int) and isinstance(first_timestamp, int)
            else None,
            "stop_and_flush": round((stop_finished - dom_final_finished) * 1000, 3),
            "total": round((stop_finished - run_started) * 1000, 3),
        },
        "verification": {
            "matches": matches[:12],
            "initial_match_count": len(initial_matches),
            "refreshed_match_count": len(final_matches),
            "minimum_matches": args.minimum_matches,
            "sample_dom_rows": dom_final["quotes"][:8],
            "sample_received_payloads": [payload_text(record)[:300] for record in received[:4]],
        },
    }


def close_candidate(candidate: subprocess.Popen[Any]) -> str:
    if candidate.poll() is not None:
        return "already_exited"
    if platform.system() == "Linux":
        windows = subprocess.run(
            ["xdotool", "search", "--onlyvisible", "--pid", str(candidate.pid)],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            check=False,
        ).stdout.split()
        if windows:
            subprocess.run(["xdotool", "windowactivate", "--sync", windows[0]], check=False)
            subprocess.run(["xdotool", "key", "alt+F4"], check=False)
            return "linux_alt_f4"
    elif platform.system() == "Darwin":
        script = (
            'tell application "System Events" to tell '
            f'(first process whose unix id is {candidate.pid}) to perform action "AXClose" of window 1'
        )
        if subprocess.run(["osascript", "-e", script], check=False).returncode == 0:
            return "macos_ax_close"
    os.killpg(candidate.pid, 15)
    return "task_owned_terminate_fallback"


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    browser_smoke.validate_platform(args.backend)
    if not args.allow_closed_market and not market_is_open():
        raise SystemExit("E24 live market-data smoke requires Oslo market hours (weekdays 09:00-16:20)")
    repo_root = Path(__file__).resolve().parents[2]
    command = browser_smoke.resolve_executable(args.horizon if args.horizon.is_absolute() else repo_root / args.horizon)
    changes = browser_smoke.git_changes(repo_root)
    if changes and not args.allow_dirty:
        raise SystemExit("E24 final evidence requires a clean exact-head checkout; use --allow-dirty only while iterating")
    root = browser_smoke.create_root(args.root)
    for child in ["logs", "profiles", "proof"]:
        (root / child).mkdir(parents=True, exist_ok=True)
    config = write_config(args, root)
    environment = browser_smoke.smoke_environment(root)
    environment["RUST_LOG"] = "off"
    run_started = time.monotonic()
    candidate_log = root / "logs" / f"e24-{args.backend}-horizon.log"
    result: dict[str, Any]
    close_mode = "not_started"
    tracker: browser_smoke.ProcessTracker | None = None
    with candidate_log.open("w", encoding="utf-8") as log:
        candidate = subprocess.Popen(
            [str(command), "--config", str(config), "--ephemeral"],
            env=environment,
            stdout=log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        tracker = browser_smoke.ProcessTracker(candidate.pid)
        tracker.start()
        print(json.dumps({"candidate_pid": candidate.pid, "root": str(root)}), flush=True)
        client = mcp_client(command, root / "logs" / f"e24-{args.backend}-mcp.log", environment, args.backend)
        try:
            result = exercise(client, args, run_started)
        except Exception as error:  # preserve a machine-readable failure artifact
            result = {
                "backend": args.backend,
                "error": str(error),
                "failures": ["unhandled E24 smoke failure"],
                "passed": False,
                "traceback": traceback.format_exc(),
            }
        finally:
            client.close()
        result["git_dirty"] = bool(changes)
        (root / "e24-result.json").write_text(json.dumps(result, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
        if args.keep_open:
            print(json.dumps({"instruction": "Inspect and close the exact E24 smoke window", "root": str(root)}), flush=True)
        else:
            close_mode = close_candidate(candidate)
        try:
            candidate_status = candidate.wait(timeout=300 if args.keep_open else 20)
        except subprocess.TimeoutExpired:
            os.killpg(candidate.pid, 15)
            candidate_status = candidate.wait(timeout=10)
            close_mode = "task_owned_timeout_terminate"
    assert tracker is not None
    tracker.stop()
    report = {
        **result,
        "candidate_status": candidate_status,
        "close_mode": close_mode,
        "remaining_manifests": browser_smoke.active_manifests(root),
        "root": str(root),
        "surviving_browser_processes": sorted(
            set(tracker.survivors() + browser_smoke.root_path_survivors(root))
        ),
    }
    report["passed"] = bool(
        result.get("passed")
        and candidate_status == 0
        and close_mode not in {"task_owned_terminate_fallback", "task_owned_timeout_terminate"}
        and not report["remaining_manifests"]
        and not report["surviving_browser_processes"]
    )
    report_path = root / "report.json"
    report_path.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(json.dumps({**report, "report": str(report_path)}, ensure_ascii=False, sort_keys=True), flush=True)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
