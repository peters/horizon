#!/usr/bin/env python3
"""Run a live E24 market-data correctness and timing gate through Horizon MCP."""

from __future__ import annotations

import argparse
import base64
import json
import math
import os
import platform
import re
import subprocess
import sys
import time
import traceback
from dataclasses import dataclass, field
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
    parser.add_argument(
        "--summary-interval-seconds",
        type=float,
        help="emit and verify incremental market summaries at this interval",
    )
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


def wait_for_panel(
    client: mcp_gate.McpClient,
    backend: str,
    timeout_seconds: float,
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout_seconds
    while time.monotonic() < deadline:
        listed, _ = client.call("browser_list", {})
        assert listed is not None
        matching = [panel for panel in listed["panels"] if panel["backend"] == backend]
        if len(matching) == 1:
            return matching[0]
        if len(matching) > 1:
            raise AssertionError(f"E24 smoke found multiple {backend} browser panels: {matching}")
        time.sleep(0.1)
    raise AssertionError(f"E24 smoke did not discover its {backend} browser panel")


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
    # A watch response may deliberately return only a bounded prefix for a
    # large, unrelated market body. Never interpret partial JSONP as quotes.
    if record.get("truncated"):
        return []
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


@dataclass
class NetworkWatch:
    """Consume one Horizon-owned capture through the bounded MCP cursor contract."""

    client: mcp_gate.McpClient
    panel_id: str
    capture_id: str
    action_ids: list[str]
    retain_records: bool = False
    next_sequence: int = 0
    records: list[dict[str, Any]] = field(default_factory=list)
    calls: int = 0
    timed_out_calls: int = 0
    returned_payloads_truncated: int = 0
    sequence_gaps: int = 0
    malformed_records: int = 0
    connection_urls_truncated: int = 0
    capture_changed: bool = False
    file_reset: bool = False
    records_dropped: int = 0
    payloads_truncated: int = 0
    file_limit_reached: bool = False
    writer_failed: bool = False

    def poll(self, wait_seconds: float) -> list[dict[str, Any]]:
        wait_millis = max(1, min(60_000, math.ceil(wait_seconds * 1000)))
        result = mcp_gate.record_action(
            self.client,
            "browser_network_watch",
            {
                "panel_id": self.panel_id,
                "capture_id": self.capture_id,
                "after_sequence": self.next_sequence,
                "wait_millis": wait_millis,
                "max_records": 250,
                "url_patterns": ["mws.fcgi"],
                "event_kinds": ["http_response_body"],
                "include_payload": True,
                "max_payload_bytes": 64 * 1024,
                "timeout_millis": 60_000,
            },
            self.action_ids,
        )
        if result["capture_id"] != self.capture_id:
            raise AssertionError(
                f"E24 capture changed from {self.capture_id} to {result['capture_id']}"
            )
        self.next_sequence = int(result["next_sequence"])
        self.calls += 1
        self.timed_out_calls += int(bool(result["timed_out"]))
        self.returned_payloads_truncated += int(result["returned_payloads_truncated"])
        self.sequence_gaps += int(result["sequence_gaps"])
        self.malformed_records += int(result["malformed_records"])
        self.connection_urls_truncated += int(result["connection_urls_truncated"])
        self.capture_changed |= bool(result["capture_changed"])
        self.file_reset |= bool(result["file_reset"])
        self.records_dropped = max(self.records_dropped, int(result["records_dropped"]))
        self.payloads_truncated = max(self.payloads_truncated, int(result["payloads_truncated"]))
        self.file_limit_reached |= bool(result["file_limit_reached"])
        self.writer_failed |= bool(result["writer_failed"])
        records = result["records"]
        if self.retain_records:
            self.records.extend(records)
        return records

    def health(self) -> dict[str, Any]:
        return {
            "calls": self.calls,
            "timed_out_calls": self.timed_out_calls,
            "next_sequence": self.next_sequence,
            "returned_payloads_truncated": self.returned_payloads_truncated,
            "sequence_gaps": self.sequence_gaps,
            "malformed_records": self.malformed_records,
            "connection_urls_truncated": self.connection_urls_truncated,
            "capture_changed": self.capture_changed,
            "file_reset": self.file_reset,
            "records_dropped": self.records_dropped,
            "payloads_truncated": self.payloads_truncated,
            "file_limit_reached": self.file_limit_reached,
            "writer_failed": self.writer_failed,
        }


@dataclass
class MarketState:
    latest: dict[str, tuple[int, dict[str, Any]]] = field(default_factory=dict)
    previous_prices: dict[str, Decimal] = field(default_factory=dict)
    records: int = 0
    response_bodies: int = 0
    response_body_bytes: int = 0
    response_body_errors: int = 0
    response_body_truncations: int = 0
    stock_updates: int = 0

    def ingest(self, records: list[dict[str, Any]]) -> tuple[dict[str, int], set[str]]:
        counts = {"new_response_bodies": 0, "new_stock_updates": 0}
        instruments: set[str] = set()
        for record in records:
            self.records += 1
            kind = str(record.get("kind", ""))
            if kind != "http_response_body":
                continue
            counts["new_response_bodies"] += 1
            self.response_bodies += 1
            self.response_body_bytes += int(record.get("payload_bytes", 0) or 0)
            self.response_body_errors += int(bool(record.get("error")))
            if record.get("truncated"):
                self.response_body_truncations += 1
                continue
            timestamp = int(record.get("timestamp_millis", 0) or 0)
            for item in feed_items(record):
                price = json_decimal(item.get("lastprice"))
                instrument = str(item.get("insref", ""))
                if not instrument or price is None or item.get("instrumenttype") != 4:
                    continue
                self.latest[instrument] = (timestamp, item)
                instruments.add(instrument)
                self.stock_updates += 1
                counts["new_stock_updates"] += 1
        return counts, instruments

    @staticmethod
    def compact(entry: tuple[int, dict[str, Any]]) -> dict[str, Any]:
        _, item = entry
        return {
            "symbol": item.get("symbol", ""),
            "name": item.get("name", ""),
            "price": str(json_decimal(item.get("lastprice"))),
            "day_change_percent": str(json_decimal(item.get("diff1dprc"))),
        }

    def interval_summary(
        self,
        interval: int,
        elapsed_seconds: float,
        new_counts: dict[str, int],
    ) -> dict[str, Any]:
        current_prices = {
            instrument: price
            for instrument, (_, item) in self.latest.items()
            if (price := json_decimal(item.get("lastprice"))) is not None
        }
        changed = []
        for instrument in current_prices.keys() & self.previous_prices.keys():
            before = self.previous_prices[instrument]
            after = current_prices[instrument]
            if before == after:
                continue
            _, item = self.latest[instrument]
            delta = after - before
            changed.append(
                {
                    "symbol": item.get("symbol", ""),
                    "name": item.get("name", ""),
                    "from": str(before),
                    "to": str(after),
                    "delta": str(delta),
                    "delta_percent": (
                        str((delta / before * 100).quantize(Decimal("0.0001"))) if before else None
                    ),
                }
            )
        self.previous_prices = current_prices
        movers = sorted(
            (entry for entry in self.latest.values() if json_decimal(entry[1].get("diff1dprc")) is not None),
            key=lambda entry: json_decimal(entry[1].get("diff1dprc")) or Decimal(0),
        )
        latest_timestamp = max((entry[0] for entry in self.latest.values()), default=0)
        return {
            "interval": interval,
            "elapsed_seconds": round(elapsed_seconds, 3),
            **new_counts,
            "captured_response_bodies": self.response_bodies,
            "bounded_response_bodies": self.response_body_truncations,
            "captured_stock_updates": self.stock_updates,
            "current_stocks": len(self.latest),
            "latest_feed_age_ms": (
                max(0, int(time.time() * 1000) - latest_timestamp) if latest_timestamp else None
            ),
            "changed_since_previous": sorted(
                changed,
                key=lambda item: abs(Decimal(item["delta_percent"] or "0")),
                reverse=True,
            )[:5],
            "top_gainers": [self.compact(entry) for entry in reversed(movers[-3:])],
            "top_losers": [self.compact(entry) for entry in movers[:3]],
        }


def compare_latest_quotes(
    dom: dict[str, Any],
    state: MarketState,
    fresh_instruments: set[str],
) -> dict[str, Any]:
    matches = []
    mismatches = []
    for quote in dom["quotes"]:
        href = quote.get("href", "")
        instrument_match = re.search(r"/aksjer/(\d+)/", href) if isinstance(href, str) else None
        if instrument_match is None:
            continue
        instrument = instrument_match.group(1)
        if instrument not in fresh_instruments:
            continue
        latest = state.latest.get(instrument)
        if latest is None:
            continue
        _, item = latest
        dom_price = decimal_value(str(quote.get("price", "")))
        dom_change = decimal_value(str(quote.get("change", "")))
        feed_price = json_decimal(item.get("lastprice"))
        feed_change = json_decimal(item.get("diff1dprc"))
        evidence = {
            "insref": instrument,
            "symbol": item.get("symbol") or quote.get("symbol", ""),
            "dom_price": str(dom_price),
            "feed_price": str(feed_price),
            "dom_change": str(dom_change),
            "feed_change": str(feed_change),
        }
        (matches if dom_price == feed_price and dom_change == feed_change else mismatches).append(evidence)
    return {
        "dom_rows": len(dom["quotes"]),
        "exact_matches": len(matches),
        "fresh_feed_instruments": len(fresh_instruments),
        "compared": len(matches) + len(mismatches),
        "sample_matches": matches[:3],
        "sample_mismatches": mismatches[:3],
    }


def wait_for_market_batch(
    watch: NetworkWatch,
    state: MarketState,
    timeout_seconds: float,
) -> tuple[dict[str, int], set[str]]:
    deadline = time.monotonic() + timeout_seconds
    totals = {"new_response_bodies": 0, "new_stock_updates": 0}
    instruments: set[str] = set()
    while time.monotonic() < deadline:
        remaining = max(0.001, deadline - time.monotonic())
        counts, updated = state.ingest(watch.poll(remaining))
        instruments.update(updated)
        for key, value in counts.items():
            totals[key] += value
        if totals["new_stock_updates"] > 0:
            counts, updated = state.ingest(watch.poll(0.25))
            instruments.update(updated)
            for key, value in counts.items():
                totals[key] += value
            return totals, instruments
    raise AssertionError("fresh E24 market response did not arrive through the browser capture")


def observe_until(watch: NetworkWatch, state: MarketState, deadline: float) -> dict[str, int]:
    totals = {"new_response_bodies": 0, "new_stock_updates": 0}
    while time.monotonic() < deadline:
        records = watch.poll(max(0.001, deadline - time.monotonic()))
        counts, _ = state.ingest(records)
        for key, value in counts.items():
            totals[key] += value
    return totals


def reload_with_fallback(
    client: mcp_gate.McpClient,
    panel_id: str,
    url: str,
    action_ids: list[str],
    failed_action_ids: list[str],
) -> dict[str, str] | None:
    try:
        mcp_gate.record_action(
            client,
            "browser_act",
            {"panel_id": panel_id, "action": "reload", "timeout_millis": 60_000},
            action_ids,
        )
        return None
    except AssertionError as error:
        failed_action_id = mcp_gate.failed_action_id(str(error))
        failed_action_ids.append(failed_action_id)
        mcp_gate.record_action(
            client,
            "browser_navigate",
            {"panel_id": panel_id, "url": url, "timeout_millis": 60_000},
            action_ids,
        )
        return {
            "failed_action_id": failed_action_id,
            "fallback": "browser_navigate",
            "reason": str(error),
        }

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


def verify_audit(
    client: mcp_gate.McpClient,
    panel_id: str,
    action_ids: list[str],
    failed_action_ids: Sequence[str] = (),
) -> dict[str, Any]:
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
    missing_failed = [
        action_id for action_id in failed_action_ids if "failed" not in statuses.get(action_id, set())
    ]
    return {
        "entries": len(audit["entries"]),
        "missing_failed_states": missing_failed,
        "missing_terminal_states": missing,
    }

def exercise(client: mcp_gate.McpClient, args: argparse.Namespace, run_started: float) -> dict[str, Any]:
    mcp_gate.initialize(client)
    action_ids: list[str] = []
    panel = wait_for_panel(client, args.backend, 90)
    panel_ready = time.monotonic()
    panel_id = panel["panel_id"]
    detail, _ = client.call("browser_panel", {"panel_id": panel_id})
    assert detail is not None
    if not detail["network_capture"]["supported"]:
        raise AssertionError(f"network capture is unavailable: {detail['network_capture']}")
    if detail["network_capture"].get("http_response_body_transport") not in {"cdp", "webdriver_bidi"}:
        raise AssertionError(f"native HTTP response bodies are unavailable: {detail['network_capture']}")
    started_at = time.monotonic()
    capture_started_wall_millis = int(time.time() * 1000)
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
    watch = NetworkWatch(client, panel_id, started["capture_id"], action_ids, retain_records=True)
    state = MarketState()
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
        wait_for_market_batch(watch, state, args.feed_timeout_seconds)
        first_payload_seen = time.monotonic()
        dom_start = evaluate_quotes(client, panel_id, action_ids)
        observe_until(watch, state, time.monotonic() + args.observation_seconds)
        refresh_boundary = watch.next_sequence
        refresh_started = time.monotonic()
        mcp_gate.record_action(
            client,
            "browser_act",
            {"panel_id": panel_id, "action": "reload", "timeout_millis": 60_000},
            action_ids,
        )
        refresh_finished = time.monotonic()
        wait_for_market_batch(watch, state, args.feed_timeout_seconds)
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
    records = watch.records
    decode_started = time.monotonic()
    received = [record for record in records if record.get("kind") == "http_response_body"]
    complete_received = [record for record in received if not record.get("truncated")]
    first_payload = complete_received[0] if complete_received else None
    refreshed_payload = next(
        (
            record
            for record in complete_received
            if int(record.get("sequence", 0)) > refresh_boundary
        ),
        None,
    )
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
    watch_health_failures = [
        key
        for key in [
            "capture_changed",
            "file_reset",
            "sequence_gaps",
            "malformed_records",
            "connection_urls_truncated",
            "records_dropped",
            "payloads_truncated",
            "file_limit_reached",
            "writer_failed",
        ]
        if watch.health()[key] not in (0, False)
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
    if watch_health_failures:
        failures.append(f"network watch health failed: {', '.join(watch_health_failures)}")
    if audit["missing_terminal_states"]:
        failures.append("one or more MCP actions lack complete audit states")
    first_timestamp = first_payload.get("timestamp_millis") if first_payload else None
    refreshed_timestamp = refreshed_payload.get("timestamp_millis") if refreshed_payload else None
    return {
        "audit": audit,
        "backend": args.backend,
        "capture": {
            "bytes_written": stopped["bytes_written"],
            "connections": stopped["known_connections"],
            "path": str(capture_path),
            "response_body_bytes": sum(record.get("payload_bytes", 0) or 0 for record in received),
            "response_bodies": len(received),
            "bounded_response_bodies": len(received) - len(complete_received),
            "records": stopped["records_written"],
            "transport": stopped["transport"],
            "watch": watch.health(),
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
                first_timestamp - capture_started_wall_millis
                if isinstance(first_timestamp, int)
                else None
            ),
            "decode_and_match": round((decode_finished - decode_started) * 1000, 3),
            "dom_final_query": round((dom_final_finished - dom_final_started) * 1000, 3),
            "horizon_launch_to_panel_ready": round((panel_ready - run_started) * 1000, 3),
            "navigation": round((navigation_finished - navigation_started) * 1000, 3),
            "panel_ready_to_capture_start": round((started_at - panel_ready) * 1000, 3),
            "feed_wait": round((first_payload_seen - dom_ready) * 1000, 3),
            "refresh": round((refresh_finished - refresh_started) * 1000, 3),
            "refresh_feed_wait": round((refresh_payload_seen - refresh_finished) * 1000, 3),
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
            "sample_received_payloads": [payload_text(record)[:300] for record in complete_received[:4]],
        },
    }


def exercise_periodic(
    client: mcp_gate.McpClient,
    args: argparse.Namespace,
    run_started: float,
) -> dict[str, Any]:
    mcp_gate.initialize(client)
    action_ids: list[str] = []
    failed_action_ids: list[str] = []
    panel = wait_for_panel(client, args.backend, 90)
    panel_id = panel["panel_id"]
    detail, _ = client.call("browser_panel", {"panel_id": panel_id})
    assert detail is not None
    expected_transport = "cdp" if args.backend == "chromium" else "webdriver_bidi"
    actual_transport = detail["network_capture"].get("http_response_body_transport")
    if actual_transport != expected_transport:
        raise AssertionError(f"unexpected HTTP body transport: {detail['network_capture']}")

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
    watch = NetworkWatch(client, panel_id, started["capture_id"], action_ids)
    stopped: dict[str, Any] | None = None
    state = MarketState()
    summaries: list[dict[str, Any]] = []
    reload_retries: list[dict[str, Any]] = []
    try:
        mcp_gate.record_action(
            client,
            "browser_navigate",
            {"panel_id": panel_id, "url": args.url, "timeout_millis": 60_000},
            action_ids,
        )
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
        wait_for_market_batch(watch, state, args.feed_timeout_seconds)
        state.previous_prices = {
            instrument: price
            for instrument, (_, item) in state.latest.items()
            if (price := json_decimal(item.get("lastprice"))) is not None
        }
        monitor_started = time.monotonic()
        interval_seconds = float(args.summary_interval_seconds)
        expected_intervals = math.ceil(args.observation_seconds / interval_seconds)
        for interval in range(1, expected_intervals + 1):
            deadline = monitor_started + min(args.observation_seconds, interval * interval_seconds)
            observe_until(watch, state, deadline)
            reload_started = time.monotonic()
            boundary_delay_ms = max(0.0, (reload_started - deadline) * 1000)
            retry = reload_with_fallback(
                client,
                panel_id,
                args.url,
                action_ids,
                failed_action_ids,
            )
            if retry is not None:
                reload_retries.append({"interval": interval, **retry})
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
            reload_finished = time.monotonic()
            new_counts, fresh_instruments = wait_for_market_batch(
                watch,
                state,
                args.feed_timeout_seconds,
            )
            feed_seen = time.monotonic()
            dom = evaluate_quotes(client, panel_id, action_ids)
            comparison = compare_latest_quotes(dom, state, fresh_instruments)
            status = mcp_gate.record_action(
                client,
                "browser_network",
                {"panel_id": panel_id, "operation": "status", "timeout_millis": 30_000},
                action_ids,
            )
            summary = {
                "kind": "e24_market_summary",
                "backend": args.backend,
                **state.interval_summary(interval, time.monotonic() - monitor_started, new_counts),
                "schedule_timing_ms": {
                    "boundary_delay": round(boundary_delay_ms, 3),
                    "published_after_boundary": round((time.monotonic() - deadline) * 1000, 3),
                },
                "refresh_timing_ms": {
                    "reload_and_table": round((reload_finished - reload_started) * 1000, 3),
                    "fresh_feed_after_table": round((feed_seen - reload_finished) * 1000, 3),
                },
                "dom_verification": comparison,
                "capture_health": {
                    "records_written": status["records_written"],
                    "bytes_written": status["bytes_written"],
                    "records_dropped": status["records_dropped"],
                    "payloads_truncated": status["payloads_truncated"],
                    "file_limit_reached": status["file_limit_reached"],
                    "writer_failed": status["writer_failed"],
                    "network_watch": watch.health(),
                },
                "reload_retry": retry,
            }
            summaries.append(summary)
            print(json.dumps(summary, ensure_ascii=False, sort_keys=True), flush=True)

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
    verify_jq(capture_path)
    audit = verify_audit(client, panel_id, action_ids, failed_action_ids)
    expected_intervals = math.ceil(args.observation_seconds / float(args.summary_interval_seconds))
    failures = []
    if len(summaries) != expected_intervals:
        failures.append(f"emitted {len(summaries)} summaries instead of {expected_intervals}")
    weak_intervals = [
        summary["interval"]
        for summary in summaries
        if summary["dom_verification"]["exact_matches"] < args.minimum_matches
    ]
    if weak_intervals:
        failures.append(f"DOM/feed exact-match threshold missed in intervals {weak_intervals}")
    late_intervals = [
        summary["interval"]
        for summary in summaries
        if max(summary["schedule_timing_ms"].values()) >= float(args.summary_interval_seconds) * 1000
    ]
    if late_intervals:
        failures.append(f"summary publication fell at least one full interval behind in {late_intervals}")
    active_intervals = sum(summary["new_response_bodies"] > 0 for summary in summaries)
    if active_intervals != expected_intervals:
        failures.append(
            f"market feed produced new response bodies in {active_intervals} intervals; "
            f"need {expected_intervals}"
        )
    if state.response_body_errors:
        failures.append(f"{state.response_body_errors} response bodies could not be retrieved")
    health_failures = [
        key
        for key in ["records_dropped", "payloads_truncated", "file_limit_reached", "writer_failed"]
        if stopped.get(key) not in (0, False)
    ]
    if health_failures:
        failures.append(f"capture health failed: {', '.join(health_failures)}")
    watch_health_failures = [
        key
        for key in [
            "capture_changed",
            "file_reset",
            "sequence_gaps",
            "malformed_records",
            "connection_urls_truncated",
            "records_dropped",
            "payloads_truncated",
            "file_limit_reached",
            "writer_failed",
        ]
        if watch.health()[key] not in (0, False)
    ]
    if watch_health_failures:
        failures.append(f"network watch health failed: {', '.join(watch_health_failures)}")
    if audit["missing_terminal_states"] or audit["missing_failed_states"]:
        failures.append("one or more MCP actions lack complete audit states")
    return {
        "audit": audit,
        "backend": args.backend,
        "capture": {
            "active_summary_intervals": active_intervals,
            "bytes_written": stopped["bytes_written"],
            "path": str(capture_path),
            "records": stopped["records_written"],
            "response_bodies": state.response_bodies,
            "bounded_response_bodies": state.response_body_truncations,
            "response_body_bytes": state.response_body_bytes,
            "stock_updates": state.stock_updates,
            "stocks": len(state.latest),
            "transport": stopped["transport"],
            "watch": watch.health(),
        },
        "failures": failures,
        "git_head": browser_smoke.git_head(Path(__file__).resolve().parents[2]),
        "passed": not failures,
        "reload_retries": reload_retries,
        "summaries": summaries,
        "total_seconds": round(time.monotonic() - run_started, 3),
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
            f'(first process whose unix id is {candidate.pid}) to click button 1 of window 1'
        )
        if subprocess.run(
            ["osascript", "-e", script],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        ).returncode == 0:
            return "macos_ax_close"
    os.killpg(candidate.pid, 15)
    return "task_owned_terminate_fallback"


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    browser_smoke.validate_platform(args.backend)
    if args.observation_seconds < 0:
        raise SystemExit("--observation-seconds cannot be negative")
    if args.summary_interval_seconds is not None:
        if args.summary_interval_seconds <= 0:
            raise SystemExit("--summary-interval-seconds must be positive")
        if args.observation_seconds <= 0:
            raise SystemExit("periodic summaries require a positive --observation-seconds")
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
            if args.summary_interval_seconds is None:
                result = exercise(client, args, run_started)
            else:
                result = exercise_periodic(client, args, run_started)
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
