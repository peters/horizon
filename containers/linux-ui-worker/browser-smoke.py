#!/usr/bin/env python3
"""Run inside the candidate's agent panel using only public browser MCP tools."""

import argparse
import http.server
import json
import os
from pathlib import Path
import sys
import threading
import time


PAGE = b"""<!doctype html><title>Worker browser smoke</title>
<h1>Worker browser smoke</h1><label>Name <input id="name"></label>
<button id="submit" onclick="document.querySelector('#result').textContent=
'Hello '+document.querySelector('#name').value">Submit</button><p id="result"></p>"""


class Fixture(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(PAGE)))
        self.end_headers()
        self.wfile.write(PAGE)

    def log_message(self, _format, *_args):
        pass


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--repository", type=Path, required=True)
    parser.add_argument("--backend", choices=("chromium", "firefox"), required=True)
    parser.add_argument("--artifacts", type=Path, required=True)
    args = parser.parse_args()
    # Reuse the repository's public MCP transport; no raw browser endpoint,
    # private runtime-file discovery or browser-control CLI is involved.
    sys.path.insert(0, str(args.repository / "scripts/browser-smoke"))
    from mcp_gate import McpClient

    result = {"backend": args.backend, "status": "failed", "checks": []}
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Fixture)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    client = None
    try:
        client = McpClient(
            args.binary, args.artifacts / "browser-mcp.log", 60,
            os.environ["HORIZON_BROWSER_ACTOR"], os.environ["HORIZON_BROWSER_HOST_INSTANCE"],
        )
        initialized = client.request("initialize", {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "clientInfo": {"name": "horizon-worker-smoke", "version": "1"},
        })
        if initialized["result"]["serverInfo"]["name"] != "horizon-browser":
            raise RuntimeError("unexpected MCP server")
        client.notify("notifications/initialized")
        listed, _ = client.call("browser_list", {})
        if listed["panels"]:
            raise RuntimeError("fixture already owns a browser panel")
        created, _ = client.call("browser_create", {
            "backend": args.backend, "url": f"http://127.0.0.1:{server.server_port}/",
            "visible": True, "timeout_millis": 60000,
        })
        panel = created["panel"]["panel_id"]
        if created["navigation"] != "committed":
            raise RuntimeError("fixture navigation did not commit")
        result["checks"].append("created_and_navigated")
        client.call("browser_snapshot", {"panel_id": panel, "max_nodes": 30})
        field, _ = client.call("browser_query", {"panel_id": panel, "selector": "#name"})
        client.call("browser_act", {
            "panel_id": panel, "action": "fill", "ref": field["nodes"][0]["ref"], "value": "worker",
        })
        button, _ = client.call("browser_query", {"panel_id": panel, "selector": "#submit"})
        client.call("browser_act", {"panel_id": panel, "action": "click", "ref": button["nodes"][0]["ref"]})
        observed, _ = client.call("browser_wait", {
            "panel_id": panel, "selector": "#result", "state": "visible", "timeout_millis": 5000,
        })
        if observed["nodes"][0]["text"] != "Hello worker":
            raise RuntimeError("form submission did not produce the expected result")
        result["checks"].append("filled_clicked_and_observed")
        result["status"] = "passed"
    except Exception as error:
        result["error"] = str(error)
    finally:
        if client is not None:
            try:
                client.close()
            except Exception as error:
                result.update(status="failed", error=str(error))
        server.shutdown()
        server.server_close()
        pending = args.artifacts / "browser-result.pending"
        pending.write_text(json.dumps(result, indent=2) + "\n")
        pending.rename(args.artifacts / "browser-result.json")
    # Keep this actor and its panel identity alive until the supervisor captures
    # the visible result and closes its own candidate through the window manager.
    while True:
        time.sleep(1)


if __name__ == "__main__":
    main()
