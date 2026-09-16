#!/usr/bin/env python3
"""Unattended HTTP Basic/Digest smoke for Chromium and Firefox via public MCP."""

from __future__ import annotations

import json
import os
import platform
import subprocess
import sys
import time
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

sys.dont_write_bytecode = True
import fixture_server
import mcp_gate
import run as browser_smoke


AUTH_USER = fixture_server.AUTH_USER
AUTH_PASSWORD = fixture_server.AUTH_PASSWORD
WRONG_PASSWORD = "wrong-pass-zephyr"


def mcp_client(
    command: Path,
    log: Path,
    environment: dict[str, str],
    actor: str,
    host_instance: str,
) -> mcp_gate.McpClient:
    original = os.environ.copy()
    os.environ.clear()
    os.environ.update(environment)
    try:
        return mcp_gate.McpClient(command, log, 90, actor, host_instance)
    finally:
        os.environ.clear()
        os.environ.update(original)


def wait_for_actor(root: Path, timeout: float = 30) -> tuple[str, str]:
    actor_path = root / "agent-actor"
    host_path = root / "agent-host-instance"
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            actor = actor_path.read_text(encoding="utf-8").strip()
        except OSError:
            actor = ""
        if actor.startswith("horizon:"):
            host = host_path.read_text(encoding="utf-8").strip()
            if not host:
                raise SystemExit("agent panel did not receive HORIZON_BROWSER_HOST_INSTANCE")
            return actor, host
        time.sleep(0.1)
    raise SystemExit("agent panel did not publish its Horizon browser actor")


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
            f"(first process whose unix id is {candidate.pid}) to click button 1 of window 1"
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


def fixture_self_check(base_url: str) -> None:
    for scheme, args in (
        ("basic", ["--basic"]),
        ("digest", ["--digest"]),
    ):
        completed = subprocess.run(
            [
                "curl",
                "-fsS",
                *args,
                "-u",
                f"{AUTH_USER}:{AUTH_PASSWORD}",
                f"{base_url}/{scheme}-auth",
            ],
            check=False,
            capture_output=True,
            text=True,
        )
        if completed.returncode != 0 or f"authenticated-{scheme}-zephyr" not in completed.stdout:
            raise SystemExit(
                f"fixture {scheme} auth self-check failed: status={completed.returncode} "
                f"stderr={completed.stderr.strip()} body={completed.stdout[:200]!r}"
            )
        denied = subprocess.run(
            ["curl", "-sS", f"{base_url}/{scheme}-auth"],
            check=False,
            capture_output=True,
            text=True,
        )
        if "authenticated-" in denied.stdout:
            raise SystemExit(f"fixture {scheme} auth allowed an anonymous request")


def origin_of(base_url: str) -> str:
    parsed = urlsplit(base_url)
    return f"{parsed.scheme}://{parsed.netloc}"


def other_loopback_url(base_url: str) -> str:
    if "127.0.0.1" in base_url:
        return base_url.replace("127.0.0.1", "localhost", 1)
    return base_url.replace("localhost", "127.0.0.1", 1)


def set_http_auth(
    client: mcp_gate.McpClient,
    panel_id: str,
    password: str,
    origin: str,
) -> dict[str, Any]:
    result, _ = client.call(
        "browser_http_auth",
        {
            "panel_id": panel_id,
            "operation": "set",
            "username": AUTH_USER,
            "password": password,
            "origin": origin,
            "timeout_millis": 15_000,
        },
    )
    assert result is not None
    if not result.get("active") or result.get("operation") != "set":
        raise AssertionError(f"HTTP auth set did not activate: {result}")
    return result


def marker_text(client: mcp_gate.McpClient, panel_id: str) -> str:
    result, _ = client.call(
        "browser_evaluate",
        {
            "panel_id": panel_id,
            "expression": "(document.getElementById('auth-marker') && document.getElementById('auth-marker').textContent) || ''",
        },
    )
    assert result is not None
    value = result.get("value")
    if isinstance(value, str):
        return value
    return ""


def navigate(client: mcp_gate.McpClient, panel_id: str, url: str) -> dict[str, Any]:
    result, _ = client.call(
        "browser_navigate",
        {"panel_id": panel_id, "url": url, "timeout_millis": 15_000},
    )
    assert result is not None
    return result


def wait_for_marker(client: mcp_gate.McpClient, panel_id: str, expected: str) -> str:
    waited, _ = client.call(
        "browser_wait",
        {
            "panel_id": panel_id,
            "selector": "#auth-marker",
            "state": "present",
            "timeout_millis": 15_000,
        },
    )
    assert waited is not None
    text = marker_text(client, panel_id)
    if text != expected:
        raise AssertionError(f"protected page marker mismatch: expected {expected!r} got {text!r} wait={waited}")
    return text


def exercise(client: mcp_gate.McpClient, args: Any) -> dict[str, Any]:
    mcp_gate.initialize(client)
    listed, _ = client.call("browser_list", {})
    assert listed is not None and not listed["panels"]
    created, _ = client.call(
        "browser_create",
        {
            "url": f"{args.base_url}/index.html",
            "backend": args.backend,
            "visible": False,
            "timeout_millis": 45_000,
        },
    )
    assert created is not None
    panel_id = created["panel"]["panel_id"]
    origin = origin_of(args.base_url)

    anonymous = navigate(client, panel_id, f"{args.base_url}/basic-auth")
    if marker_text(client, panel_id) != "":
        raise AssertionError(f"anonymous Basic navigation reached the protected marker: {anonymous}")

    wrong = set_http_auth(client, panel_id, WRONG_PASSWORD, origin)
    navigate(client, panel_id, f"{args.base_url}/basic-auth")
    if marker_text(client, panel_id) != "":
        raise AssertionError("wrong Basic credentials reached the protected marker")

    set_http_auth(client, panel_id, AUTH_PASSWORD, origin)
    navigate(client, panel_id, f"{args.base_url}/basic-auth")
    basic = wait_for_marker(client, panel_id, "authenticated-basic-zephyr")
    navigate(client, panel_id, f"{args.base_url}/digest-auth")
    digest = wait_for_marker(client, panel_id, "authenticated-digest-zephyr")

    other_url = other_loopback_url(args.base_url)
    if other_url == args.base_url:
        raise AssertionError(f"could not derive a distinct loopback origin from {args.base_url}")
    other = navigate(client, panel_id, f"{other_url}/basic-auth")
    if marker_text(client, panel_id) != "":
        raise AssertionError(f"origin-scoped credentials authenticated a different origin: {other}")

    cleared, _ = client.call(
        "browser_http_auth",
        {"panel_id": panel_id, "operation": "clear", "timeout_millis": 15_000},
    )
    assert cleared is not None and cleared.get("active") is False
    cleared_other = navigate(client, panel_id, f"{other_url}/digest-auth")
    if marker_text(client, panel_id) != "":
        raise AssertionError(f"cleared credentials authenticated a distinct origin: {cleared_other}")

    audit, _ = client.call("browser_audit", {"panel_id": panel_id, "limit": 200})
    assert audit is not None
    encoded = json.dumps(audit)
    if AUTH_PASSWORD in encoded or WRONG_PASSWORD in encoded:
        raise AssertionError("audit recorded an HTTP auth password")
    http_auth_entries = [entry for entry in audit["entries"] if entry.get("action", {}).get("type") == "http_auth"]
    if len(http_auth_entries) < 3:
        raise AssertionError(f"audit did not record HTTP auth set/clear: {http_auth_entries}")
    if any("password" in json.dumps(entry) and AUTH_PASSWORD in json.dumps(entry) for entry in http_auth_entries):
        raise AssertionError("HTTP auth audit leaked a password")

    return {
        "backend": args.backend,
        "basic_marker": basic,
        "digest_marker": digest,
        "http_auth_audit_entries": len(http_auth_entries),
        "origin": origin,
        "panel_id": panel_id,
        "wrong_credentials_rejected": True,
        "anonymous_rejected": True,
        "cross_origin_rejected": True,
        "cleared": True,
        "set_action_id": wrong.get("action_id"),
    }


def main(argv: list[str] | None = None) -> int:
    args = browser_smoke.parse_args(argv)
    browser_smoke.validate_platform(args.backend)
    if args.backend not in {"chromium", "firefox"}:
        raise SystemExit("HTTP auth smoke covers Chromium and Firefox")
    repo_root = Path(__file__).resolve().parents[2]
    command = browser_smoke.resolve_executable(
        args.horizon if args.horizon.is_absolute() else repo_root / args.horizon
    )
    changes = browser_smoke.git_changes(repo_root)
    if changes and not args.allow_dirty:
        raise SystemExit("HTTP auth smoke requires a clean exact-head checkout; use --allow-dirty only while iterating")
    root = browser_smoke.create_root(args.root)
    for child in ["logs", "profiles", "proof"]:
        (root / child).mkdir(parents=True, exist_ok=True)
    server, server_thread, base_url = browser_smoke.start_fixture_server(root)
    fixture_self_check(base_url)
    config = browser_smoke.write_config(args, root)
    environment = browser_smoke.smoke_environment(root)
    horizon_log = root / "logs" / f"{args.backend}-http-auth-horizon.log"
    result: dict[str, Any] = {"backend": args.backend, "passed": False}
    close_mode = "not_started"
    candidate_status = 1
    candidate: subprocess.Popen[Any] | None = None
    tracker: browser_smoke.ProcessTracker | None = None
    launch = [str(command), "--config", str(config)]
    if args.ephemeral:
        launch.append("--ephemeral")
    try:
        with horizon_log.open("w", encoding="utf-8") as log:
            candidate = subprocess.Popen(
                launch,
                env=environment,
                stdout=log,
                stderr=subprocess.STDOUT,
                start_new_session=True,
            )
            tracker = browser_smoke.ProcessTracker(candidate.pid)
            tracker.start()
            print(json.dumps({"candidate_pid": candidate.pid, "root": str(root)}, sort_keys=True), flush=True)
            actor, host_instance = wait_for_actor(root)
            client = mcp_client(
                command,
                root / "logs" / f"{args.backend}-http-auth-mcp.log",
                environment,
                actor,
                host_instance,
            )
            try:
                args.base_url = base_url
                result = exercise(client, args)
                result["passed"] = True
            except Exception as error:  # noqa: BLE001 — machine-readable smoke artifact
                result = {"backend": args.backend, "error": str(error), "passed": False}
            finally:
                client.close()
    finally:
        if candidate is not None:
            close_mode = close_candidate(candidate)
            try:
                candidate_status = candidate.wait(timeout=20)
            except subprocess.TimeoutExpired:
                os.killpg(candidate.pid, 15)
                candidate_status = candidate.wait(timeout=10)
                close_mode = "task_owned_timeout_terminate"
        if tracker is not None:
            tracker.stop()
        server.shutdown()
        server.server_close()
        server_thread.join(timeout=5)

    report = {
        **result,
        "candidate_status": candidate_status,
        "close_mode": close_mode,
        "git_dirty": bool(changes),
        "git_head": browser_smoke.git_head(repo_root),
        "remaining_manifests": browser_smoke.active_manifests(root),
        "root": str(root),
        "surviving_browser_processes": sorted(
            set((tracker.survivors() if tracker is not None else []) + browser_smoke.root_path_survivors(root))
        ),
    }
    report["passed"] = bool(
        result.get("passed")
        and candidate_status == 0
        and close_mode not in {"task_owned_terminate_fallback", "task_owned_timeout_terminate"}
        and not report["remaining_manifests"]
        and not report["surviving_browser_processes"]
    )
    (root / "http-auth-result.json").write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(json.dumps(report, ensure_ascii=False, sort_keys=True), flush=True)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
