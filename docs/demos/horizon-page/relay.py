#!/usr/bin/env python3
"""Tiny Horizon Page inbox: IRC-shaped JSON over HTTP. Stdlib only."""

from __future__ import annotations

import argparse
import json
import os
import posixpath
import sys
import uuid
from datetime import datetime, timezone
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

ROOT = os.path.dirname(os.path.abspath(__file__))
MAX_BODY = 64 * 1024
MAX_TEXT = 4000
INBOX: list[dict] = []


def utc_now() -> str:
    return datetime.now(timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


class Handler(SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=ROOT, **kwargs)

    def end_headers(self) -> None:
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
        self.send_header("Access-Control-Allow-Headers", "Content-Type")
        self.send_header("Cache-Control", "no-store")
        super().end_headers()

    def do_OPTIONS(self) -> None:  # noqa: N802
        self.send_response(204)
        self.end_headers()

    def do_GET(self) -> None:  # noqa: N802
        parsed = urlparse(self.path)
        if parsed.path in ("/health", "/health/"):
            return self._json(200, {"ok": True, "messages": len(INBOX)})
        if parsed.path in ("/inbox", "/inbox/"):
            nick = parse_qs(parsed.query).get("nick", ["peters"])[0].strip() or "peters"
            items = [item for item in INBOX if item["to"] == nick]
            return self._json(200, items)
        if parsed.path in ("/msg", "/msg/"):
            return self._json(405, {"error": "POST a PRIVMSG to /msg"})
        return super().do_GET()

    def do_POST(self) -> None:  # noqa: N802
        parsed = urlparse(self.path)
        if parsed.path not in ("/msg", "/msg/"):
            self.send_error(404)
            return
        length = int(self.headers.get("Content-Length", "0") or "0")
        if length <= 0 or length > MAX_BODY:
            return self._json(400, {"error": "body must be 1..65536 bytes"})
        raw = self.rfile.read(length)
        try:
            payload = json.loads(raw.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError):
            return self._json(400, {"error": "invalid json"})
        message, error = normalize_privmsg(payload)
        if error:
            return self._json(400, {"error": error})
        INBOX.append(message)
        return self._json(201, message)

    def translate_path(self, path: str) -> str:
        parsed = urlparse(path)
        rel = posixpath.normpath(parsed.path).lstrip("/")
        if not rel or rel == ".":
            rel = "index.html"
        full = os.path.normpath(os.path.join(ROOT, rel))
        if full != ROOT and not full.startswith(ROOT + os.sep):
            return os.path.join(ROOT, "index.html")
        if os.path.isdir(full):
            return os.path.join(full, "index.html")
        return full

    def _json(self, status: int, payload: object) -> None:
        body = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, fmt: str, *args) -> None:
        sys.stderr.write("%s - %s\n" % (self.log_date_time_string(), fmt % args))


def normalize_privmsg(payload: object) -> tuple[dict | None, str | None]:
    if not isinstance(payload, dict):
        return None, "object required"
    cmd = str(payload.get("cmd", "PRIVMSG")).strip().upper()
    if cmd != "PRIVMSG":
        return None, "cmd must be PRIVMSG"
    from_nick = str(payload.get("from", "")).strip()
    to_nick = str(payload.get("to", "peters")).strip() or "peters"
    text = str(payload.get("text", "")).strip()
    if not from_nick or len(from_nick) > 32:
        return None, "from must be 1..32 chars"
    if len(to_nick) > 32:
        return None, "to must be 1..32 chars"
    if not text or len(text) > MAX_TEXT:
        return None, "text must be 1..4000 chars"
    if any("\n" in part or "\r" in part for part in (from_nick, to_nick)):
        return None, "nicks must be a single line"
    return {
        "id": str(payload.get("id") or uuid.uuid4()),
        "cmd": "PRIVMSG",
        "from": from_nick,
        "to": to_nick,
        "text": text,
        "ts": str(payload.get("ts") or utc_now()),
    }, None


def main() -> None:
    parser = argparse.ArgumentParser(description="Horizon Page inbox (PRIVMSG JSON)")
    parser.add_argument("--host", default="127.0.0.1", help="bind address (0.0.0.0 for a reachable host)")
    parser.add_argument("--port", type=int, default=8787)
    args = parser.parse_args()
    server = ThreadingHTTPServer((args.host, args.port), Handler)
    print(f"horizon-page relay http://{args.host}:{args.port}/  POST /msg  GET /inbox?nick=peters")
    server.serve_forever()


if __name__ == "__main__":
    main()
