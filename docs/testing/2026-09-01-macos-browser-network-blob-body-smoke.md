# macOS browser network Blob body smoke (issue 353)

Temporary cross-machine plan for the fix to
[#353](https://github.com/peters/horizon/issues/353). Run every lane on the
exact requested head. The scenario starts its own isolated hidden Chromium
through the candidate `horizon-browser mcp --standalone` binary with a private
`HOME`; it never lists, claims, or navigates a panel that belongs to a running
Horizon. That standalone process is the same stdio MCP server Horizon
registers for agents (see `crates/horizon-browser-cli/README.md`), so the
driver below stays inside the browser-control contract: it only calls MCP
tools and never touches raw CDP or private runtime files.

## What changed

With `browser_network start` plus `include_http: true` and
`include_http_bodies: true` on Chromium, a `fetch()` response the page drained
with `response.blob()` used to be written as a successful zero-byte `text`
`http_response_body` record. Chromium's `Network.getResponseBody` cannot
return those bytes. The driver now keeps bounded per-response evidence
(status, method, MIME type, `Content-Length`, header bytes, observed body
bytes, and the finished `encodedDataLength`) and writes such bodies as records
with an `error` and no `payload`. Genuinely empty responses (`Content-Length:
0`, an empty chunked body, `204`, `304`, `HEAD`) still produce a captured
zero-byte body.

## Preconditions

- A clean checkout of the requested PR commit and the macOS build
  prerequisites from `AGENTS.md`.
- Chromium or Google Chrome installed so the standalone backend can start.
- Python 3 and `curl` on `PATH`.
- TCP port 18353 free on `127.0.0.1`.

Record the candidate:

```bash
git rev-parse HEAD
git status --short
```

The SHA must match the newest smoke request and the checkout must be clean.

## Focused package gate

```bash
cargo fmt --all -- --check
./scripts/check-maintainability.sh
RUSTFLAGS="-D warnings" cargo test -p horizon-browser http_bodies
RUSTFLAGS="-D warnings" cargo test -p horizon-browser
cargo build -p horizon-browser-cli
```

The `session::http_bodies` tests must pass (eight tests) and the crate suite
must stay green.

## Fixture server

Create a scratch directory outside the repository, for example
`mktemp -d /tmp/horizon-353.XXXXXX`, and save the server there as
`server.py`. It generates a ~33 KB PDF in memory and serves it with and
without `Content-Length`, plus genuinely empty bodies.

```python
#!/usr/bin/env python3
"""Deterministic fixture server for issue 353: PDF with and without Content-Length, empty bodies."""
import os, sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

def build_pdf():
    objs = ["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>"]
    stream = "BT /F1 24 Tf 72 720 Td (Horizon issue 353 fixture) Tj ET\n" + ("% padding line for a realistic file size\n" * 800)
    objs.append(f"<< /Length {len(stream)} >>\nstream\n{stream}endstream")
    out, offsets = "%PDF-1.4\n", []
    for i, body in enumerate(objs, start=1):
        offsets.append(len(out)); out += f"{i} 0 obj\n{body}\nendobj\n"
    xref = len(out)
    out += f"xref\n0 {len(objs)+1}\n0000000000 65535 f \n" + "".join(f"{o:010d} 00000 n \n" for o in offsets)
    out += f"trailer\n<< /Size {len(objs)+1} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n"
    return out.encode("latin-1")

PDF = build_pdf()
HTML = b"<!doctype html><title>issue 353 fixture</title><h1>issue 353 fixture</h1>"

class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *args):
        pass
    def _send(self, status, ctype, body, chunked=False, extra=()):
        self.send_response(status)
        if ctype:
            self.send_header("Content-Type", ctype)
        for k, v in extra:
            self.send_header(k, v)
        if chunked:
            self.send_header("Transfer-Encoding", "chunked")
            self.end_headers()
            if body:
                self.wfile.write(b"%x\r\n" % len(body) + body + b"\r\n")
            self.wfile.write(b"0\r\n\r\n")
        else:
            if status not in (204, 304):
                self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            if body and self.command != "HEAD":
                self.wfile.write(body)
    def do_HEAD(self):
        self.do_GET()
    def do_GET(self):
        path = self.path.split("?")[0]
        if path in ("/", "/index.html"):
            self._send(200, "text/html; charset=utf-8", HTML)
        elif path == "/fixture.pdf":
            self._send(200, "application/pdf", PDF, extra=[("Content-Disposition", "inline")])
        elif path == "/chunked.pdf":
            self._send(200, "application/pdf", PDF, chunked=True, extra=[("Content-Disposition", "inline")])
        elif path == "/empty.json":
            self._send(200, "application/json", b"")
        elif path == "/empty-chunked.json":
            self._send(200, "application/json", b"", chunked=True)
        elif path == "/nocontent":
            self._send(204, None, b"")
        else:
            self._send(404, "text/plain", b"not found")

if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 18353
    ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
```

Start it and confirm the endpoints:

```bash
python3 server.py 18353 &
curl -s -o /dev/null -w "%{http_code} %{size_download}
" http://127.0.0.1:18353/fixture.pdf   # 200 and > 30000
curl -sI http://127.0.0.1:18353/chunked.pdf | grep -i transfer-encoding                            # chunked
curl -s -o /dev/null -w "%{http_code}
" http://127.0.0.1:18353/nocontent                        # 204
```

## Scenario driver

Save the driver next to the server as `smoke353.py`. It speaks JSON-RPC to
the standalone MCP server over stdio, starts a capture with bodies enabled,
navigates to the fixture page, runs the fetch variants from the issue plus the
empty-body controls, stops the capture, prints every `http_response_body`
record, and copies the NDJSON export before the isolated profile is removed.

```python
#!/usr/bin/env python3
"""Drive a standalone horizon-browser MCP server through the issue-353 scenario and print body records."""
import json, os, subprocess, sys, time, tempfile, shutil

binary = sys.argv[1]
label = sys.argv[2] if len(sys.argv) > 2 else "candidate"
copy_to = sys.argv[3] if len(sys.argv) > 3 else None
base = "http://127.0.0.1:18353"
home = tempfile.mkdtemp(prefix=f"horizon-353-{label}-")
env = {k: v for k, v in os.environ.items() if not k.startswith("CLAUDE_CODE") and k not in ("HORIZON", "HORIZON_BROWSER_ACTOR")}
env.update({"HOME": home, "RUST_LOG": "off"})
proc = subprocess.Popen([binary, "mcp", "--standalone", "--backend", "chromium"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=open(os.path.join(home, "stderr.log"), "wb"), env=env, text=True)
next_id = [0]

def rpc(method, params=None):
    next_id[0] += 1
    msg = {"jsonrpc": "2.0", "id": next_id[0], "method": method}
    if params is not None:
        msg["params"] = params
    proc.stdin.write(json.dumps(msg) + "\n"); proc.stdin.flush()
    while True:
        line = proc.stdout.readline()
        if not line:
            raise SystemExit(f"server exited; see {home}/stderr.log")
        reply = json.loads(line)
        if reply.get("id") == next_id[0]:
            return reply

def call(tool, args):
    reply = rpc("tools/call", {"name": tool, "arguments": args})
    result = reply.get("result", {})
    if reply.get("error") or result.get("isError"):
        raise SystemExit(f"{tool} failed: {json.dumps(reply)[:800]}")
    return result.get("structuredContent") or json.loads(result["content"][0]["text"])

rpc("initialize", {"protocolVersion": "2026-07-28", "capabilities": {}, "clientInfo": {"name": "issue-353-smoke", "version": "1"}})
proc.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n"); proc.stdin.flush()
panel = None
for _ in range(60):
    panels = call("browser_list", {}).get("panels", [])
    if panels:
        panel = panels[0]["panel_id"]; break
    time.sleep(0.5)
if not panel:
    raise SystemExit("no standalone panel appeared")
print(f"[{label}] panel {panel} backend {panels[0].get('backend')}")
cap = call("browser_network", {"panel_id": panel, "operation": "start", "include_http": True, "include_http_bodies": True, "include_websocket": False, "max_payload_bytes": 1048576, "timeout_millis": 30000})
path = cap["capture"]["path"] if "capture" in cap else cap["path"]
print(f"[{label}] capture {path}")
call("browser_navigate", {"panel_id": panel, "url": f"{base}/index.html", "timeout_millis": 30000})
call("browser_wait", {"panel_id": panel, "selector": "h1", "state": "present", "timeout_millis": 30000})
cases = [
    ("A arrayBuffer", "new Promise(res => fetch('/fixture.pdf?case=arraybuffer').then(r => r.arrayBuffer()).then(b => res('A:' + b.byteLength)))"),
    ("B unread",      "new Promise(res => fetch('/fixture.pdf?case=unread').then(r => res('B:' + r.status)))"),
    ("C blob",        "new Promise(res => fetch('/fixture.pdf?case=blob').then(r => r.blob()).then(b => res('C:' + b.size)))"),
    ("D chunked blob","new Promise(res => fetch('/chunked.pdf?case=blob').then(r => r.blob()).then(b => res('D:' + b.size)))"),
    ("E empty CL0",   "new Promise(res => fetch('/empty.json?case=text').then(r => r.text()).then(t => res('E:' + t.length)))"),
    ("F empty chunked","new Promise(res => fetch('/empty-chunked.json?case=text').then(r => r.text()).then(t => res('F:' + t.length)))"),
    ("G 204",         "new Promise(res => fetch('/nocontent?case=text').then(r => r.text()).then(t => res('G:' + t.length)))"),
    ("H HEAD",        "new Promise(res => fetch('/fixture.pdf?case=head', {method: 'HEAD'}).then(r => res('H:' + r.status)))"),
]
for name, expr in cases:
    out = call("browser_evaluate", {"panel_id": panel, "expression": expr, "timeout_millis": 30000})
    print(f"[{label}] {name}: {json.dumps(out.get('value', out))[:80]}")
    time.sleep(0.6)
time.sleep(1.5)
call("browser_network", {"panel_id": panel, "operation": "stop", "timeout_millis": 30000})
print(f"[{label}] http_response_body records:")
with open(path) as f:
    for line in f:
        rec = json.loads(line)
        if rec.get("kind") != "http_response_body":
            continue
        payload = rec.get("payload")
        print("  ", json.dumps({
            "url": rec.get("url"), "encoding": rec.get("payload_encoding"), "payload_bytes": rec.get("payload_bytes"),
            "truncated": rec.get("truncated"), "payload_present": payload is not None,
            "payload_prefix": (payload or "")[:12], "error": rec.get("error")}))
if copy_to:
    shutil.copy(path, copy_to)
    print(f"[{label}] capture copied to {copy_to}")
proc.stdin.close()
try:
    proc.wait(timeout=30)
except subprocess.TimeoutExpired:
    proc.kill()
print(f"[{label}] server exit {proc.returncode}; home {home}")
```

Run it against the candidate from the repository root:

```bash
python3 /path/to/smoke353.py target/debug/horizon-browser candidate /tmp/horizon-353-capture.ndjson
```

## Expected records

The driver prints one line per `http_response_body` record. Pass criteria:

- `fixture.pdf?case=arraybuffer` and `fixture.pdf?case=unread`: `encoding`
  `base64`, `payload_bytes` equal to the PDF size reported by `curl`,
  `payload_present` true, `payload_prefix` `JVBERi0xLjQK`, `error` null.
- `fixture.pdf?case=blob`: `payload_present` false, `encoding` and
  `payload_bytes` null, `error` starting with
  `Chromium returned an empty body for a response that declared Content-Length`
  and naming `application/pdf` and `Blob`.
- `chunked.pdf?case=blob`: `payload_present` false, `error` starting with
  `Chromium returned an empty body for a response that transferred` and
  ending with the same Blob explanation.
- `empty.json`, `empty-chunked.json`, and `nocontent`: `encoding` `text`,
  `payload_bytes` 0, `payload_present` true, `error` null.
- `fixture.pdf?case=head`: Chromium reported no response lifecycle for the
  `HEAD` fetch on Linux and macOS, so normally only an `http_request` record
  with `method` `HEAD` appears and no `http_response_body` record follows. If
  Chromium does report one, it must be a captured zero-byte body (`encoding`
  `text`, `payload_bytes` 0, `payload_present` true, `error` null); an
  `error` on the `HEAD` case is a failure.
- The driver ends with `server exit 0` and the isolated profile home it
  printed no longer contains a `captures` directory.

The eight `browser_evaluate` lines must report `A:<pdf size>`, `B:200`,
`C:<pdf size>`, `D:<pdf size>`, `E:0`, `F:0`, `G:0`, and `H:200`; the page
itself receives every body, only the CDP capture of the Blob cases changes.

## Cleanup

Stop the fixture server (`kill %1` or the recorded PID), delete the scratch
directory and the copied capture, and confirm no task-owned `horizon-browser`
or Chromium process remains (`pgrep -fl "horizon-browser mcp"`).

## Report

Post the result on the PR using this shape:

```text
SMOKE-TEST REPORT (<machine/os>)
- exact head and clean checkout: pass | fail, <SHA and note>
- focused package gate: pass | fail, <note>
- fixture server endpoints: pass | fail, <note>
- Blob-drained bodies reported as unavailable: pass | fail, <both error texts>
- arrayBuffer and unread bodies captured: pass | fail, <payload_bytes>
- genuinely empty bodies still captured: pass | fail, <note>
- cleanup: pass | fail, <note>
Summary: <fixes pushed, remaining findings, or no findings>
SMOKE-TEST: DONE
```

If a finding is fixed, rerun every affected lane on the new exact head and
report only that final head.
