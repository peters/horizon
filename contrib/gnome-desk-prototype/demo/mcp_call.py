#!/usr/bin/env python3
"""Tiny MCP client for demos: runs `horizon --browser-mcp` with this panel's injected identity."""
import json, os, subprocess, sys

def call(arguments):
    p = subprocess.Popen([os.environ["HORIZON_MCP_BIN"], "--browser-mcp"], stdin=subprocess.PIPE,
                         stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
    def send(m): p.stdin.write(json.dumps(m) + "\n"); p.stdin.flush()
    def recv(want):
        while True:
            line = p.stdout.readline()
            if not line: return None
            m = json.loads(line)
            if m.get("id") == want: return m
    send({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "demo", "version": "0"}}})
    recv(1)
    send({"jsonrpc": "2.0", "method": "notifications/initialized"})
    send({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "agent_panels", "arguments": arguments}})
    reply = recv(2)
    p.terminate()
    result = (reply or {}).get("result", {})
    return result.get("structuredContent") or result or (reply or {}).get("error")

def panel_by_title(fragment):
    for panel in call({"operation": "list"}).get("panels", []):
        if fragment.lower() in panel["title"].lower() and not panel["is_caller"]:
            return panel["panel_id"]
    raise SystemExit(f"no agent titled {fragment}")

command = sys.argv[1]
if command == "list": out = call({"operation": "list"})
elif command == "send": out = call({"operation": "send", "panel_id": panel_by_title(sys.argv[2]), "text": " ".join(sys.argv[3:])})
elif command == "read": out = call({"operation": "read", "panel_id": panel_by_title(sys.argv[2]), "lines": int(sys.argv[3]) if len(sys.argv) > 3 else 12})
elif command == "note": out = call({"operation": "note", "title": sys.argv[2].replace("_", " "), "markdown": " ".join(sys.argv[3:]).replace("|", "\n")})
elif command == "plan":
    arg = sys.argv[2]
    steps = json.load(open(arg[1:])) if arg.startswith("@") else json.loads(" ".join(sys.argv[2:]))
    out = call({"operation": "plan", "steps": steps})
elif command == "approvals": out = call({"operation": "approvals"})
else: out = {"error": f"unknown command {command}"}
print(json.dumps(out)[:600])
