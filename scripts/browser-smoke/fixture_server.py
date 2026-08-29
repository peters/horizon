#!/usr/bin/env python3
"""Serve the deterministic Horizon browser smoke site on loopback only."""

from __future__ import annotations

import argparse
import base64
import functools
import hashlib
import http.server
import json
import struct
import time
from pathlib import Path
from typing import Sequence
from urllib.parse import parse_qs, urlsplit


FIXTURE_ROOT = Path(__file__).resolve().parent / "fixtures"


class SmokeHandler(http.server.SimpleHTTPRequestHandler):
    """Static handler with deterministic cache and health responses."""

    protocol_version = "HTTP/1.1"

    def end_headers(self) -> None:
        self.send_header("Cache-Control", "no-store")
        self.send_header("X-Content-Type-Options", "nosniff")
        super().end_headers()

    def do_GET(self) -> None:  # noqa: N802 - stdlib handler API
        path = self.path.split("?", 1)[0]
        if path == "/market-stream":
            self._serve_market_stream()
            return
        if path == "/slow-navigation.html":
            time.sleep(11)
            body = b"<!doctype html><title>Horizon Browser Smoke - Slow Navigation</title><p id='slow-marker'>ready</p>"
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        if path == "/healthz":
            body = b"horizon-browser-smoke\n"
            self.send_response(200)
            self.send_header("Content-Type", "text/plain; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        super().do_GET()

    def _serve_market_stream(self) -> None:
        key = self.headers.get("Sec-WebSocket-Key")
        if self.headers.get("Upgrade", "").lower() != "websocket" or not key:
            self.send_error(400, "WebSocket upgrade required")
            return
        accept = base64.b64encode(
            hashlib.sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode("ascii")).digest()
        ).decode("ascii")
        self.send_response(101, "Switching Protocols")
        self.send_header("Upgrade", "websocket")
        self.send_header("Connection", "Upgrade")
        self.send_header("Sec-WebSocket-Accept", accept)
        self.end_headers()
        self.close_connection = True
        query = parse_qs(urlsplit(self.path).query)
        try:
            frame_count = int(query.get("frames", ["4096"])[0])
        except ValueError:
            frame_count = 0
        if not 1 <= frame_count <= 4096:
            return
        attempt = query.get("attempt", ["1"])[0]
        try:
            if frame_count < 128:
                time.sleep(0.025)
            for sequence in range(frame_count):
                payload = json.dumps(
                    {
                        "attempt": attempt,
                        "price": round(100 + (sequence % 97) * 0.01, 2),
                        "sequence": sequence,
                        "symbol": "TEST",
                        "volume": 10 + sequence % 31,
                    },
                    separators=(",", ":"),
                ).encode("utf-8")
                self.connection.sendall(websocket_frame(0x1, payload))
                if frame_count < 128:
                    time.sleep(0.002)
                elif sequence % 128 == 127:
                    time.sleep(0.001)
            if frame_count < 128:
                time.sleep(0.025)
            self.connection.sendall(websocket_frame(0x8, struct.pack("!H", 1000) + b"fixture-complete"))
        except (BrokenPipeError, ConnectionResetError):
            return

    def log_message(self, message: str, *args: object) -> None:
        del message, args
        path = urlsplit(self.path).path
        print(f"fixture: {self.address_string()} {self.command} {path}", flush=True)


def create_server(port: int = 0) -> http.server.ThreadingHTTPServer:
    handler = functools.partial(SmokeHandler, directory=str(FIXTURE_ROOT))
    server = http.server.ThreadingHTTPServer(("127.0.0.1", port), handler)
    server.daemon_threads = True
    return server


def websocket_frame(opcode: int, payload: bytes) -> bytes:
    """Encode one final unmasked server-to-client WebSocket frame."""
    header = bytearray([0x80 | opcode])
    length = len(payload)
    if length < 126:
        header.append(length)
    elif length <= 0xFFFF:
        header.extend((126,))
        header.extend(struct.pack("!H", length))
    else:
        header.extend((127,))
        header.extend(struct.pack("!Q", length))
    return bytes(header) + payload


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=0, help="loopback port; zero selects a free port")
    parser.add_argument("--url-file", type=Path, help="atomically record the selected base URL")
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    server = create_server(args.port)
    base_url = f"http://127.0.0.1:{server.server_port}"
    if args.url_file:
        args.url_file.parent.mkdir(parents=True, exist_ok=True)
        temporary = args.url_file.with_suffix(args.url_file.suffix + ".tmp")
        temporary.write_text(base_url + "\n", encoding="utf-8")
        temporary.replace(args.url_file)
    print(json.dumps({"base_url": base_url, "fixture_root": str(FIXTURE_ROOT)}, sort_keys=True), flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
