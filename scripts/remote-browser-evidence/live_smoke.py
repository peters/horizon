#!/usr/bin/env python3
"""Phase 6 evidence run for #628: a real remote device session through Horizon's
public MCP tools only.

The provider credential comes from the private netrc (never arguments, never the
environment of the Horizon process) and is seeded into the Secret Service under
the exact item Horizon's keyring adapter addresses (service `horizon-remote-browser`,
user `<origin>|<slot>`), then removed at the end. Horizon runs headless with an
isolated HOME; the agent panel's identity is read from the probe file it writes.
The flow per target: browser_create {target,url}, snapshot, fill + click, query the
result, drawer open/close, frame query, browser_close; then the provider REST API
is asked whether the session is terminal (release proof), and a local screenshot
of the panel is kept.
"""

from __future__ import annotations

import argparse
import json
import netrc
import os
import pathlib
import re
import signal
import stat
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

SCRATCH = pathlib.Path(__file__).resolve().parent
REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "scripts" / "browser-smoke"))
from mcp_gate import HANDSHAKE_PROTOCOL_VERSION, McpClient  # noqa: E402


def handshake(client: McpClient) -> None:
    """Plain MCP initialize; the gate helper asserts a fixed tool list that predates browser_close."""
    response = client.request(
        "initialize",
        {"protocolVersion": HANDSHAKE_PROTOCOL_VERSION, "capabilities": {}, "clientInfo": {"name": "horizon-628-phase6", "version": "1"}},
    )
    if response["result"]["serverInfo"]["name"] != "horizon-browser":
        raise SystemExit(response)
    client.notify("notifications/initialized")


RPC_LOG: list[pathlib.Path] = []


def raw(client: McpClient, name: str, arguments: dict) -> dict:
    """The tool result as the server returned it, errors included; nothing is
    asserted here. Every exchange is appended to the run's JSON-RPC log."""
    response = client.request("tools/call", {"name": name, "arguments": arguments})
    result = {"isError": True, "content": response["error"]} if "error" in response else response.get("result", {})
    if RPC_LOG:
        with RPC_LOG[0].open("a", encoding="utf-8") as log:
            log.write(json.dumps({"tool": name, "arguments": arguments, "result": result}, ensure_ascii=False) + "\n")
    return result

NETRC = pathlib.Path("~/.config/horizon-dev/browserstack.netrc").expanduser()
HUB = "https://hub-cloud.browserstack.com/wd/hub"
HUB_ORIGIN = "https://hub-cloud.browserstack.com"
API = "https://api.browserstack.com"
FIXTURE = "https://peters.github.io/horizon-mobile-fixture/"
SERVICE = "horizon-remote-browser"
SLOTS = {"user": "remote-browser/browserstack/user", "key": "remote-browser/browserstack/key"}
TERMINAL = {"done", "completed", "passed", "failed", "timeout", "error"}
TARGETS = {
    "ios_phone": {
        "provider": "browserstack",
        "browser_name": "Safari",
        "platform_name": "iOS",
        "device": {"kind": "physical", "model": "iPhone 16", "os_version": "18"},
        "capability_extensions": {"bstack:options": {"projectName": "horizon-628", }},
    },
    "android_phone": {
        "provider": "browserstack",
        "browser_name": "Chrome",
        "platform_name": "Android",
        "device": {"kind": "physical", "model": "Google Pixel 9", "os_version": "16.0"},
        "capability_extensions": {"bstack:options": {"projectName": "horizon-628", }},
    },
}


def load_credential() -> tuple[str, str]:
    mode = stat.S_IMODE(NETRC.stat().st_mode)
    if mode & 0o077:
        raise SystemExit("netrc must be mode 600")
    entry = netrc.netrc(str(NETRC)).hosts.get("hub-cloud.browserstack.com")
    if not entry:
        raise SystemExit("netrc has no hub-cloud.browserstack.com entry")
    login, _, password = entry
    if not login or not password:
        raise SystemExit("netrc entry incomplete")
    return login, password


def secret_schema():
    import gi

    gi.require_version("Secret", "1")
    from gi.repository import Secret

    schema = Secret.Schema.new(
        "org.freedesktop.Secret.Generic",
        Secret.SchemaFlags.NONE,
        {"service": Secret.SchemaAttributeType.STRING, "username": Secret.SchemaAttributeType.STRING},
    )
    return Secret, schema


def slot_attributes(reference: str) -> dict[str, str]:
    return {"service": SERVICE, "username": f"{HUB_ORIGIN}|{SLOTS[reference]}"}


def seed_keyring(login: str, password: str) -> dict[str, str | None]:
    """Store the run's credential under Horizon's slots and hand back whatever
    those slots held before, so the user's own binding survives the run."""
    Secret, schema = secret_schema()
    previous = {reference: Secret.password_lookup_sync(schema, slot_attributes(reference), None) for reference in SLOTS}
    for reference, value in (("user", login), ("key", password)):
        attrs = slot_attributes(reference)
        try:
            stored = Secret.password_store_sync(schema, attrs, Secret.COLLECTION_DEFAULT, f"keyring:{attrs['username']}@{SERVICE}", value, None)
        except Exception:
            # Seeding is all or nothing: whatever was already replaced goes back.
            restore_keyring(previous)
            raise
        if not stored:
            restore_keyring(previous)
            raise SystemExit(f"could not seed the OS store for {reference}")
    return previous


def restore_keyring(previous: dict[str, str | None]) -> None:
    """Put back the values the slots held before the run, or clear them."""
    Secret, schema = secret_schema()
    for reference, value in previous.items():
        attrs = slot_attributes(reference)
        if value is None:
            Secret.password_clear_sync(schema, attrs, None)
        else:
            Secret.password_store_sync(schema, attrs, Secret.COLLECTION_DEFAULT, f"keyring:{attrs['username']}@{SERVICE}", value, None)


def target_with_session_name(name: str, session_name: str) -> dict:
    """The configured target with a run-unique provider session name, so the
    release proof can query exactly this run's session."""
    target = json.loads(json.dumps(TARGETS[name]))
    target["capability_extensions"]["bstack:options"]["sessionName"] = session_name
    return target


def write_config(root: pathlib.Path, targets: list[str], session_names: dict[str, str]) -> pathlib.Path:
    actor_path = root / "agent-actor"
    host_path = root / "agent-host-instance"
    probe = (
        "import os,pathlib,time;"
        f"pathlib.Path({str(host_path)!r}).write_text(os.environ.get('HORIZON_BROWSER_HOST_INSTANCE',''),encoding='utf-8');"
        f"pathlib.Path({str(actor_path)!r}).write_text(os.environ['HORIZON_BROWSER_ACTOR'],encoding='utf-8');"
        "time.sleep(3600)"
    )
    config = {
        "version": 10,
        "window": {"width": 1400, "height": 900},
        "browser": {
            "remote": {
                "providers": {
                    "browserstack": {
                        "adapter": "browserstack",
                        "endpoint": HUB,
                        "authentication": {"kind": "basic", "username_ref": "user", "password_ref": "key"},
                        "credential_bindings": {
                            "user": {"store": "os_keychain", "slot": SLOTS["user"]},
                            "key": {"store": "os_keychain", "slot": SLOTS["key"]},
                        },
                        "limits": {
                            "max_sessions": 1,
                            "allocation_timeout_seconds": 180,
                            "idle_release_seconds": 180,
                            "max_session_seconds": 900,
                        },
                    }
                },
                "targets": {name: target_with_session_name(name, session_names[name]) for name in targets},
            }
        },
        "workspaces": [
            {
                "name": "Remote device evidence",
                "position": [30, 30],
                "terminals": [
                    {
                        "name": "Evidence agent",
                        "kind": "codex",
                        "command": sys.executable,
                        "args": ["-c", probe],
                        "position": [30, 30],
                        "size": [620, 360],
                    }
                ],
            }
        ],
    }
    path = root / "config.json"
    path.write_text(json.dumps(config, indent=2) + "\n", encoding="utf-8")
    return path


def wait_for(path: pathlib.Path, timeout: float) -> str:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.exists() and path.read_text(encoding="utf-8").strip():
            return path.read_text(encoding="utf-8").strip()
        time.sleep(0.5)
    raise SystemExit(f"timed out waiting for {path.name}")


def shot(display: str, path: pathlib.Path) -> None:
    subprocess.run(["import", "-window", "root", str(path)], env={**os.environ, "DISPLAY": display}, check=False)


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """A redirect would resend the Basic credential to wherever it points;
    refuse it and treat the response as the error it is."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):  # noqa: D102
        return None


OPENER = urllib.request.build_opener(NoRedirect())


def api_get(url: str, auth: str) -> dict:
    if not url.startswith(API + "/"):
        raise SystemExit(f"refusing to send the credential to {url}")
    req = urllib.request.Request(url, headers={"Authorization": auth, "Accept": "application/json"})
    try:
        with OPENER.open(req, timeout=60) as resp:
            return {"status": resp.status, "body": json.loads(resp.read().decode("utf-8") or "{}")}
    except urllib.error.HTTPError as err:
        return {"status": err.code, "body": {}}
    except (urllib.error.URLError, OSError) as err:
        return {"status": None, "error": type(err).__name__}


def provider_session_state(auth: str, session_name: str, wait_seconds: float = 45.0) -> dict:
    """Release proof: the newest session named by the target must be terminal at
    the provider. The record can lag the DELETE by a few seconds, so this polls."""
    deadline = time.monotonic() + wait_seconds
    last: dict = {"status": None, "terminal": None, "note": "no session with that name found"}
    while time.monotonic() < deadline:
        builds = api_get(f"{API}/automate/builds.json?limit=5", auth)
        for item in builds.get("body") or []:
            build = item.get("automation_build", {}) if isinstance(item, dict) else {}
            sessions = api_get(f"{API}/automate/builds/{build.get('hashed_id')}/sessions.json", auth)
            for entry in sessions.get("body") or []:
                session = entry.get("automation_session", {}) if isinstance(entry, dict) else {}
                if session.get("name") == session_name:
                    last = {"status": session.get("status"), "terminal": session.get("status") in TERMINAL,
                            "device": session.get("device"), "os_version": session.get("os_version"),
                            "duration": session.get("duration"), "build": build.get("name")}
                    if last["terminal"]:
                        return last
                    break
            else:
                continue
            break
        time.sleep(5)
    return last


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--horizon", required=True)
    parser.add_argument("--targets", nargs="+", default=["ios_phone"])
    parser.add_argument("--display", default=":99")
    parser.add_argument("--out", default=str(pathlib.Path("~/.cache/horizon-628-spike/phase6").expanduser()))
    args = parser.parse_args()
    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    root = out / f"run-{int(time.time())}"
    session_names = {name: f"phase6-{name}-{root.name}" for name in TARGETS}
    (root / "home").mkdir(parents=True)
    login, password = load_credential()
    auth = "Basic " + __import__("base64").b64encode(f"{login}:{password}".encode()).decode()
    report: dict = {"targets": {}, "started": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}
    unknown = [name for name in args.targets if name not in TARGETS]
    if unknown:
        raise SystemExit(f"unknown targets: {', '.join(unknown)} (known: {', '.join(TARGETS)})")
    # The MCP server must resolve the same isolated Horizon home as the host,
    # so the isolation is applied to this process too: McpClient copies the
    # environment it is launched from.
    os.environ["HOME"] = str(root / "home")
    os.environ["DISPLAY"] = args.display
    os.environ["RUST_LOG"] = "info"
    os.environ.pop("HORIZON_BROWSER_ACTOR", None)
    os.environ.pop("HORIZON", None)
    env = dict(os.environ)
    config = write_config(root, args.targets, session_names)
    log = (root / "horizon.log").open("w", encoding="utf-8")
    # Everything fallible before this point ran with the user's keyring
    # untouched; from the seed on, the restoration guard is already active.
    app = None
    # A termination signal must unwind through the finally below so the
    # keyring is restored and Horizon is stopped; Python's default would
    # exit at once.
    for sig in (signal.SIGTERM, signal.SIGHUP):
        signal.signal(sig, lambda signum, frame: (_ for _ in ()).throw(SystemExit(128 + signum)))
    previous = seed_keyring(login, password)
    try:
        app = subprocess.Popen([args.horizon, "--config", str(config), "--ephemeral"], env=env, stdout=log, stderr=log)
        actor = wait_for(root / "agent-actor", 90)
        host_instance = wait_for(root / "agent-host-instance", 30)
        time.sleep(3)
        for target in args.targets:
            steps: list[dict] = []
            client = McpClient(pathlib.Path(args.horizon), root / f"mcp-{target}.log", 200.0, actor, host_instance)
            RPC_LOG[:] = [root / f"rpc-{target}.jsonl"]
            try:
                handshake(client)
                started = time.monotonic()
                created = raw(client, "browser_create", {"target": target, "url": FIXTURE, "timeout_millis": 60000})
                steps.append({"step": "browser_create", "ms": int((time.monotonic() - started) * 1000),
                              "is_error": created.get("isError"), "result": created.get("structuredContent") or created.get("content")})
                panel = (created.get("structuredContent") or {}).get("panel") or {}
                panel_id = panel.get("panel_id")
                if not panel_id:
                    report["targets"][target] = {"steps": steps}
                    continue
                steps.append({"step": "panel_advertises", "remote_target": panel.get("remote_target"),
                              "protocol": panel.get("protocol"), "network_capture": (panel.get("network_capture") or {}).get("supported")})
                time.sleep(2)
                shot(args.display, root / f"{target}-01-created.png")
                snap = raw(client, "browser_snapshot", {"panel_id": panel_id, "max_nodes": 60})
                nodes = ((snap.get("structuredContent") or {}).get("nodes") or [])
                steps.append({"step": "browser_snapshot", "is_error": snap.get("isError"), "node_count": len(nodes),
                              "title": (snap.get("structuredContent") or {}).get("title")})
                def ref_for(pred):
                    for node in nodes:
                        if pred(node):
                            return node.get("ref")
                    return None
                metrics = raw(client, "browser_evaluate", {"panel_id": panel_id, "expression":
                    "JSON.stringify({ua: navigator.userAgent, touch: navigator.maxTouchPoints, screen: [screen.width, screen.height],"
                    " inner: [innerWidth, innerHeight], dpr: devicePixelRatio, platform: navigator.platform})"})
                steps.append({"step": "device_metrics", "is_error": metrics.get("isError"),
                              "value": (metrics.get("structuredContent") or {}).get("value")})
                fill = raw(client, "browser_act", {"panel_id": panel_id, "action": "fill", "selector": "#name", "value": "Horizon 628"})
                typed = raw(client, "browser_evaluate", {"panel_id": panel_id, "expression": "document.getElementById('name').value"})
                steps.append({"step": "fill_name", "is_error": fill.get("isError"),
                              "field_value": (typed.get("structuredContent") or {}).get("value")})
                shot(args.display, root / f"{target}-02-keyboard.png")
                viewport = raw(client, "browser_evaluate", {"panel_id": panel_id, "expression":
                    "JSON.stringify({scrollY: window.scrollY, visual: [visualViewport.offsetTop, visualViewport.pageTop, visualViewport.height], inner: window.innerHeight, submit: document.getElementById('submit').getBoundingClientRect().toJSON(), active: document.activeElement && document.activeElement.id})"})
                steps.append({"step": "viewport_before_submit", "value": (viewport.get("structuredContent") or {}).get("value")})
                click = raw(client, "browser_act", {"panel_id": panel_id, "action": "click", "selector": "#submit"})
                steps.append({"step": "click_submit", "is_error": click.get("isError")})
                waited = raw(client, "browser_wait", {"panel_id": panel_id, "selector": "#result", "state": "visible", "timeout_millis": 10000})
                steps.append({"step": "wait_result", "is_error": waited.get("isError"),
                              "elapsed_millis": (waited.get("structuredContent") or {}).get("elapsed_millis")})
                result = raw(client, "browser_evaluate", {"panel_id": panel_id, "expression": "document.getElementById('result').textContent"})
                result_value = (result.get("structuredContent") or {}).get("value")
                steps.append({"step": "result_text", "is_error": result.get("isError"), "value": result_value})
                if result_value == "result:none":
                    # Diagnostic: dismiss the on-screen keyboard first, then click again.
                    blur = raw(client, "browser_evaluate", {"panel_id": panel_id, "expression": "document.activeElement && document.activeElement.blur(); 'blurred'"})
                    time.sleep(1.5)
                    viewport = raw(client, "browser_evaluate", {"panel_id": panel_id, "expression":
                        "JSON.stringify({scrollY: window.scrollY, visual: [visualViewport.offsetTop, visualViewport.pageTop, visualViewport.height], inner: window.innerHeight, submit: document.getElementById('submit').getBoundingClientRect().toJSON(), active: document.activeElement && document.activeElement.id})"})
                    click = raw(client, "browser_act", {"panel_id": panel_id, "action": "click", "selector": "#submit"})
                    time.sleep(1.0)
                    result = raw(client, "browser_evaluate", {"panel_id": panel_id, "expression": "document.getElementById('result').textContent"})
                    steps.append({"step": "submit_retry_after_blur", "is_error": blur.get("isError") or click.get("isError"),
                                  "viewport": (viewport.get("structuredContent") or {}).get("value"),
                                  "value": (result.get("structuredContent") or {}).get("value")})
                shot(args.display, root / f"{target}-03-submitted.png")
                drawer = raw(client, "browser_act", {"panel_id": panel_id, "action": "click", "selector": "#open-drawer"})
                # The fixture opens and closes the drawer by toggling its `open`
                # class (it keeps a box while closed), so the wait targets that state.
                drawer_wait = raw(client, "browser_wait", {"panel_id": panel_id, "selector": "#drawer.open", "state": "visible", "timeout_millis": 10000})
                steps.append({"step": "drawer_open", "is_error": drawer.get("isError") or drawer_wait.get("isError")})
                shot(args.display, root / f"{target}-04-drawer.png")
                closed_drawer = raw(client, "browser_act", {"panel_id": panel_id, "action": "click", "selector": "#close-drawer"})
                closed_wait = raw(client, "browser_wait", {"panel_id": panel_id, "selector": "#drawer.open", "state": "hidden", "timeout_millis": 5000})
                method = "driver_click"
                driver_wait_error = wait_error_code(closed_wait)
                if driver_wait_error == "wait_timeout" and not closed_drawer.get("isError"):
                    # Explicit conditional result: on some devices the driver's tap
                    # misses a fixed-position control at the bottom of an inflated
                    # layout viewport (peters/horizon#663); a scripted click records
                    # that the drawer closes only through the page's own handler.
                    scripted = raw(client, "browser_evaluate", {"panel_id": panel_id, "expression": "(document.getElementById('close-drawer').click(), document.getElementById('drawer').classList.contains('open'))"})
                    closed_wait = raw(client, "browser_wait", {"panel_id": panel_id, "selector": "#drawer.open", "state": "hidden", "timeout_millis": 5000})
                    method = "scripted_click" if not scripted.get("isError") else "driver_click"
                steps.append({"step": "drawer_close", "method": method, "is_error": closed_drawer.get("isError") or closed_wait.get("isError"),
                              "driver_wait_error": driver_wait_error, "wait_error": wait_error_code(closed_wait),
                              "elapsed_millis": (closed_wait.get("structuredContent") or {}).get("elapsed_millis")})
                frame = raw(client, "browser_query", {"panel_id": panel_id, "selector": "#frame", "max_results": 1})
                steps.append({"step": "iframe_boundary", "is_error": frame.get("isError"),
                              "nodes": (frame.get("structuredContent") or {}).get("nodes")})
                scrolled = raw(client, "browser_act", {"panel_id": panel_id, "action": "scroll", "delta_y": 600})
                steps.append({"step": "scroll", "is_error": scrolled.get("isError")})
                bottom = raw(client, "browser_evaluate", {"panel_id": panel_id, "expression": "String(window.scrollY)"})
                steps.append({"step": "scroll_y", "value": (bottom.get("structuredContent") or {}).get("value")})
                shot(args.display, root / f"{target}-05-scrolled.png")
                closed = raw(client, "browser_close", {"panel_id": panel_id, "timeout_millis": 60000})
                steps.append({"step": "browser_close", "is_error": closed.get("isError"),
                              "result": closed.get("structuredContent") or closed.get("content")})
                listed = raw(client, "browser_list", {})
                steps.append({"step": "browser_list_after_close", "is_error": listed.get("isError"),
                              "panels": [p.get("panel_id") for p in ((listed.get("structuredContent") or {}).get("panels") or [])]})
            finally:
                client.close()
            time.sleep(5)
            steps.append({"step": "provider_release_proof", **provider_session_state(auth, session_names[target])})
            report["targets"][target] = {"steps": steps}
            (root / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    finally:
        if app is not None:
            app.terminate()
            try:
                app.wait(timeout=20)
            except subprocess.TimeoutExpired:
                app.kill()
        log.close()
        restore_keyring(previous)
    failures = {target: target_failures(entry["steps"]) for target, entry in report["targets"].items()}
    for target in args.targets:
        if target not in failures:
            failures[target] = ["no result recorded"]
    for target, problems in failures.items():
        report["targets"].setdefault(target, {"steps": []})["failures"] = problems
    report["passed"] = not any(failures.values())
    (root / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2))
    return 0 if report["passed"] else 1


def wait_error_code(result: dict) -> str | None:
    """The typed code of a failed browser_wait (`wait_timeout`,
    `wait_navigation_invalidated`, ...), or None when the wait succeeded."""
    if not result.get("isError"):
        return None
    text = " ".join(item.get("text", "") for item in result.get("content") or [] if isinstance(item, dict))
    match = re.search(r"failed \((\w+)\)", text)
    return match.group(1) if match else "unknown"


def target_failures(steps: list[dict]) -> list[str]:
    """What the flow must show for a target to count as evidence; every
    shortfall is named, and any shortfall makes the run fail."""
    by_step: dict[str, dict] = {}
    for step in steps:
        by_step.setdefault(step["step"], step)
    problems: list[str] = []

    def require(name: str) -> dict | None:
        step = by_step.get(name)
        if step is None:
            problems.append(f"{name}: not reached")
            return None
        if step.get("is_error"):
            problems.append(f"{name}: tool error")
            return None
        return step

    created = require("browser_create")
    if created and (created.get("result") or {}).get("navigation") != "committed":
        problems.append("browser_create: first page did not commit")
    require("browser_snapshot")
    filled = require("fill_name")
    if filled and filled.get("field_value") != "Horizon 628":
        problems.append(f"fill_name: field holds {filled.get('field_value')!r}")
    require("click_submit")
    result = require("result_text")
    if result and result.get("value") != "result:Horizon 628":
        problems.append(f"result_text: {result.get('value')!r}")
    require("drawer_open")
    require("drawer_close")
    frame = require("iframe_boundary")
    if frame and not frame.get("nodes"):
        problems.append("iframe_boundary: no iframe node")
    require("scroll")
    scrolled = by_step.get("scroll_y")
    if scrolled is None or not scrolled.get("value") or float(scrolled["value"]) <= 0:
        problems.append("scroll_y: page did not move")
    closed = require("browser_close")
    if closed and not (closed.get("result") or {}).get("closed"):
        problems.append("browser_close: not closed")
    listed = require("browser_list_after_close")
    if listed and listed.get("panels"):
        problems.append("browser_list_after_close: panel still listed")
    proof = by_step.get("provider_release_proof")
    if proof is None or not proof.get("terminal"):
        problems.append("provider_release_proof: session not terminal at the provider")
    return problems


if __name__ == "__main__":
    raise SystemExit(main())
