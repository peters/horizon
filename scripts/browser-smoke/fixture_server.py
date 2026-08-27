#!/usr/bin/env python3
"""Serve the deterministic Horizon browser smoke site on loopback only."""

from __future__ import annotations

import argparse
import functools
import http.server
import json
from pathlib import Path
from typing import Sequence
from urllib.parse import urlsplit


FIXTURE_ROOT = Path(__file__).resolve().parent / "fixtures"


class SmokeHandler(http.server.SimpleHTTPRequestHandler):
    """Static handler with deterministic cache and health responses."""

    def end_headers(self) -> None:
        self.send_header("Cache-Control", "no-store")
        self.send_header("X-Content-Type-Options", "nosniff")
        super().end_headers()

    def do_GET(self) -> None:  # noqa: N802 - stdlib handler API
        if self.path.split("?", 1)[0] == "/healthz":
            body = b"horizon-browser-smoke\n"
            self.send_response(200)
            self.send_header("Content-Type", "text/plain; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        super().do_GET()

    def log_message(self, message: str, *args: object) -> None:
        del message, args
        path = urlsplit(self.path).path
        print(f"fixture: {self.address_string()} {self.command} {path}", flush=True)


def create_server(port: int = 0) -> http.server.ThreadingHTTPServer:
    handler = functools.partial(SmokeHandler, directory=str(FIXTURE_ROOT))
    server = http.server.ThreadingHTTPServer(("127.0.0.1", port), handler)
    server.daemon_threads = True
    return server


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
