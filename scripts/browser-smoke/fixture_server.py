#!/usr/bin/env python3
"""Serve the deterministic Horizon browser smoke site on loopback only."""

from __future__ import annotations

import argparse
import base64
import functools
import hashlib
import http.server
import json
import os
import re
import struct
import threading
import time
from pathlib import Path
from typing import Sequence
from urllib.parse import parse_qs, urlsplit


FIXTURE_ROOT = Path(__file__).resolve().parent / "fixtures"
AUTH_USER = "smoke-user"
AUTH_PASSWORD = "smoke-pass-zephyr"
AUTH_REALM = "Horizon Browser Smoke"
DIGEST_PARAM = re.compile(r'([a-zA-Z][a-zA-Z0-9_-]*)\s*=\s*(?:"([^"]*)"|([^\s,]+))')


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
        if path == "/redirect-to-next":
            self.send_response(302)
            self.send_header("Location", "/next.html")
            self.send_header("Content-Length", "0")
            self.end_headers()
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
        if path == "/basic-auth":
            self._serve_basic_auth()
            return
        if path == "/digest-auth":
            self._serve_digest_auth()
            return
        if path == "/auth-child.html":
            body = b"""<!doctype html><title>Authentication child frame</title><script>
addEventListener('message', async event => {
  if (!['basic', 'digest'].includes(event.data.authScheme)) return;
  try {
    const response = await fetch('/' + event.data.authScheme + '-auth');
    const body = await response.text();
    parent.postMessage({authResult: body, status: response.status}, '*');
  } catch (error) {
    parent.postMessage({authResult: String(error), status: 0}, '*');
  }
});
parent.postMessage({authReady: true}, '*');
</script>"""
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        super().do_GET()

    def _serve_basic_auth(self) -> None:
        header = self.headers.get("Authorization", "")
        expected = "Basic " + base64.b64encode(f"{AUTH_USER}:{AUTH_PASSWORD}".encode("utf-8")).decode("ascii")
        if header == expected:
            self._send_auth_success("basic")
            return
        self._send_auth_challenge('Basic realm="%s", charset="UTF-8"' % AUTH_REALM)

    def _serve_digest_auth(self) -> None:
        header = self.headers.get("Authorization", "")
        if header.lower().startswith("digest ") and self._digest_is_valid(header):
            self._send_auth_success("digest")
            return
        nonce = base64.b64encode(os.urandom(16)).decode("ascii")
        server = getattr(self.server, "issued_nonces", None)
        lock = getattr(self.server, "auth_lock", None)
        if isinstance(server, set) and lock is not None:
            with lock:
                if len(server) >= 64:
                    server.clear()
                server.add(nonce)
        challenge = (
            f'Digest realm="{AUTH_REALM}", qop="auth", algorithm=MD5, nonce="{nonce}"'
        )
        self._send_auth_challenge(challenge)

    def _digest_is_valid(self, header: str) -> bool:
        params = parse_digest_header(header)
        if params.get("username") != AUTH_USER:
            return False
        nonce = params.get("nonce", "")
        issued = getattr(self.server, "issued_nonces", set())
        if nonce not in issued:
            return False
        uri = params.get("uri", "")
        ha1 = md5_hex(f"{AUTH_USER}:{AUTH_REALM}:{AUTH_PASSWORD}")
        ha2 = md5_hex(f"{self.command}:{uri}")
        qop = params.get("qop", "")
        if qop == "auth":
            expected = md5_hex(
                f"{ha1}:{nonce}:{params.get('nc', '')}:{params.get('cnonce', '')}:{qop}:{ha2}"
            )
        else:
            expected = md5_hex(f"{ha1}:{nonce}:{ha2}")
        return params.get("response") == expected

    def _send_auth_success(self, scheme: str) -> None:
        marker = f"authenticated-{scheme}-zephyr"
        body = (
            f"<!doctype html><title>Horizon Browser Smoke - {scheme} Auth</title>"
            f"<p id='auth-marker' data-scheme='{scheme}'>{marker}</p>"
        ).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _send_auth_challenge(self, challenge: str) -> None:
        body = (
            b"<!doctype html><title>Horizon Browser Smoke - Unauthorized</title>"
            b"<p id='auth-denied'>unauthorized-zephyr</p>"
        )
        self.send_response(401)
        self.send_header("WWW-Authenticate", challenge)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

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


class SmokeServer(http.server.ThreadingHTTPServer):
    """Loopback fixture server with a bounded Digest nonce set."""

    def __init__(self, address: tuple[str, int], handler: object) -> None:
        super().__init__(address, handler)
        self.auth_lock = threading.Lock()
        self.issued_nonces: set[str] = set()


def create_server(port: int = 0) -> SmokeServer:
    handler = functools.partial(SmokeHandler, directory=str(FIXTURE_ROOT))
    server = SmokeServer(("127.0.0.1", port), handler)
    server.daemon_threads = True
    return server


def parse_digest_header(header: str) -> dict[str, str]:
    payload = header.split(" ", 1)[1] if " " in header else header
    values: dict[str, str] = {}
    for match in DIGEST_PARAM.finditer(payload):
        values[match.group(1)] = match.group(2) if match.group(2) is not None else match.group(3)
    return values


def md5_hex(value: str) -> str:
    return hashlib.md5(value.encode("utf-8"), usedforsecurity=False).hexdigest()


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
