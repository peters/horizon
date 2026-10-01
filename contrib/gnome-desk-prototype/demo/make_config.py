#!/usr/bin/env python3
"""Writes the demo Horizon config: four rich workspaces and sixteen more, like a busy day."""
import json, sys
out = sys.argv[1]
chrome = sys.argv[2]
site = "http://127.0.0.1:8099/pricing.html"
def shell(name, pos, size, banner):
    return {"name": name, "command": "/bin/bash", "args": ["--noprofile", "--norc", "-c", f"printf '\\033[2m{banner}\\033[0m\\n'; exec /bin/bash --noprofile --norc"], "position": pos, "size": size}
rich = [
 {"name": "Api service", "cwd": "/tmp", "terminals": [
   {"name": "api-agent", "kind": "claude", "position": [0, 0], "size": [760, 560]},
   {"name": "tests", "command": "/usr/bin/python3", "args": ["-u", "-c", "import time\nprint('cargo test -p api')\nfor i in range(7200):\n print('running', i, 'tests ... ok' if i % 7 else 'FAILED clone_cleanup', flush=True); time.sleep(1)"], "position": [800, 0], "size": [760, 560]}]},
 {"name": "Marketing site", "cwd": "/tmp", "terminals": [
   {"name": "site-agent", "kind": "codex", "position": [0, 0], "size": [560, 560]},
   {"name": "pricing preview", "kind": "browser", "command": site, "position": [600, 0], "size": [760, 560]},
   {"name": "notes", "command": "/bin/bash", "args": ["--noprofile", "--norc"], "position": [1400, 0], "size": [420, 560]}]},
 {"name": "Cloud", "cwd": "/tmp", "terminals": [
   {"name": "infra-agent", "kind": "claude", "position": [0, 0], "size": [760, 560]},
   {"name": "logs", "command": "/usr/bin/python3", "args": ["-u", "-c", "import time\nfor i in range(7200):\n print('hetzner-cx42 idle for', i, 'min; auto-stop in', max(0, 120 - i), 'min', flush=True); time.sleep(2)"], "position": [800, 0], "size": [760, 560]}]},
 {"name": "Native apps", "cwd": "/tmp", "terminals": [
   {"name": "simulator", "kind": "device", "command": "127.0.0.1:5997", "position": [0, 0], "size": [760, 780]},
   {"name": "build", "command": "/usr/bin/python3", "args": ["-u", "-c", "import time\nfor i in range(7200):\n print('   Compiling app v0.4.%d (native)' % (i % 9), flush=True); time.sleep(1.5)"], "position": [800, 0], "size": [820, 780]}]},
]
others = ["Billing", "Mobile app", "Docs", "Data pipeline", "Support", "Design system", "Auth", "Search", "Admin", "Onboarding", "Analytics", "Payments", "Legacy", "Experiments", "Sandbox", "Release"]
extra = [{"name": n, "cwd": "/tmp", "terminals": [shell(n.lower().replace(" ", "-"), [0, 0], [900, 520], f"{n} workspace")]} for n in others]
config = {"version": 11, "window": {"width": 980, "height": 300}, "appearance": {"theme": "dark"},
          "browser": {"command": chrome, "headless": True, "extra_args": ["--no-sandbox", "--password-store=basic"]},
          "workspaces": rich + extra}
json.dump(config, open(out, "w"), indent=1)
print(len(config["workspaces"]), "workspaces")
