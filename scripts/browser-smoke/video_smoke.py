#!/usr/bin/env python3
"""Unattended page-pixel WebM smoke: start/pause/resume/stop plus a file-limit lane."""

from __future__ import annotations

import json
import os
import platform
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

sys.dont_write_bytecode = True
import mcp_gate
import run as browser_smoke

EBML_MAGIC = bytes([0x1A, 0x45, 0xDF, 0xA3])


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


def assert_webm(path: str) -> int:
    capture = Path(path)
    if not capture.is_absolute() or not capture.is_file():
        raise AssertionError(f"video path is not an explicit readable file: {capture}")
    if os.name != "nt" and capture.stat().st_mode & 0o777 != 0o600:
        raise AssertionError(f"video file mode is not private: {capture}")
    data = capture.read_bytes()
    if data[:4] != EBML_MAGIC or b"webm" not in data or b"V_AV1" not in data:
        raise AssertionError(f"export is not a WebM/AV1 file: {capture} ({len(data)} bytes)")
    return len(data)


def call_video(client: mcp_gate.McpClient, panel_id: str, operation: str, **options: Any) -> dict[str, Any]:
    arguments: dict[str, Any] = {
        "panel_id": panel_id,
        "operation": operation,
        "timeout_millis": 60_000,
    }
    arguments.update({key: value for key, value in options.items() if value is not None})
    result, _ = client.call("browser_video", arguments)
    assert result is not None
    return result


def wait_for_encoded_frame(client: mcp_gate.McpClient, panel_id: str, timeout: float = 45.0) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    status = call_video(client, panel_id, "status")
    while time.monotonic() < deadline:
        if status.get("frames_encoded", 0) >= 1:
            return status
        time.sleep(0.4)
        status = call_video(client, panel_id, "status")
    raise AssertionError(f"encoder did not produce a frame: {status}")


def exercise(client: mcp_gate.McpClient, args: Any) -> dict[str, Any]:
    mcp_gate.initialize(client)
    panel, _ = mcp_gate.create_panel(client, args)
    panel_id = panel["panel_id"]
    shown, _ = client.call("browser_visibility", {"panel_id": panel_id, "visible": True})
    assert shown is not None and shown["panel"]["visible"]
    navigated, _ = client.call(
        "browser_navigate",
        {"panel_id": panel_id, "url": f"{args.base_url}/index.html"},
    )
    assert navigated is not None

    started = call_video(
        client,
        panel_id,
        "start",
        quality=40,
        compression_level=0,
        fps=5,
        max_width=320,
    )
    if started["state"] != "recording" or not started["active"]:
        raise AssertionError(f"start did not begin recording: {started}")
    status = wait_for_encoded_frame(client, panel_id)
    if status["state"] != "recording":
        raise AssertionError(f"status while recording: {status}")
    paused = call_video(client, panel_id, "pause")
    if paused["state"] != "paused":
        raise AssertionError(paused)
    elapsed_at_pause = paused["elapsed_millis"]
    time.sleep(0.8)
    still_paused = call_video(client, panel_id, "status")
    paused_delta = still_paused["elapsed_millis"] - elapsed_at_pause
    if paused_delta < 0 or paused_delta > 250:
        raise AssertionError(f"pause counted idle time: delta={paused_delta}ms status={still_paused}")
    resumed = call_video(client, panel_id, "resume")
    if resumed["state"] != "recording":
        raise AssertionError(resumed)
    wait_for_encoded_frame(client, panel_id)
    stopped = call_video(client, panel_id, "stop")
    if stopped["state"] != "stopped" or stopped["active"]:
        raise AssertionError(stopped)
    bytes_written = assert_webm(stopped["path"])
    if stopped["frames_encoded"] < 1:
        raise AssertionError(f"no frames encoded: {stopped}")

    knobs = call_video(
        client,
        panel_id,
        "start",
        quality=40,
        compression_level=0,
        fps=8,
        max_width=320,
    )
    if knobs["state"] != "recording" or knobs["fps"] != 8:
        raise AssertionError(f"start overlays were not applied: {knobs}")
    wait_for_encoded_frame(client, panel_id)
    knobs_stopped = call_video(client, panel_id, "stop")
    assert_webm(knobs_stopped["path"])

    limit_bytes = 32 * 1024
    limited = call_video(
        client,
        panel_id,
        "start",
        fps=10,
        compression_level=0,
        max_width=320,
        max_file_bytes=limit_bytes,
    )
    deadline = time.monotonic() + 45
    limited_status = limited
    while time.monotonic() < deadline:
        limited_status = call_video(client, panel_id, "status")
        if limited_status.get("file_limit_reached"):
            break
        time.sleep(0.4)
    limited_stopped = call_video(client, panel_id, "stop")
    limited_size = assert_webm(limited_stopped["path"])
    if not limited_stopped.get("file_limit_reached"):
        raise AssertionError(f"file limit was not reached: {limited_stopped} bytes={limited_size}")
    if limited_size > limit_bytes + 16 * 1024:
        raise AssertionError(f"file exceeded the export bound: {limited_stopped} bytes={limited_size}")

    audit, _ = client.call("browser_audit", {"panel_id": panel_id, "limit": 200})
    assert audit is not None
    video_entries = [entry for entry in audit["entries"] if entry.get("action", {}).get("type") == "video"]
    if not video_entries:
        raise AssertionError("audit did not record video operations")
    encoded_audit = json.dumps(audit)
    if "rgb" in encoded_audit or "framebuffer" in encoded_audit:
        raise AssertionError("audit recorded page pixels")

    return {
        "backend": args.backend,
        "bytes_written": bytes_written,
        "file_limit_bytes": limited_size,
        "file_limit_reached": bool(limited_stopped.get("file_limit_reached")),
        "frames_encoded": stopped["frames_encoded"],
        "panel_id": panel_id,
        "path": stopped["path"],
        "video_audit_entries": len(video_entries),
    }


def main(argv: list[str] | None = None) -> int:
    args = browser_smoke.parse_args(argv)
    browser_smoke.validate_platform(args.backend)
    repo_root = Path(__file__).resolve().parents[2]
    command = browser_smoke.resolve_executable(
        args.horizon if args.horizon.is_absolute() else repo_root / args.horizon
    )
    changes = browser_smoke.git_changes(repo_root)
    if changes and not args.allow_dirty:
        raise SystemExit("video smoke requires a clean exact-head checkout; use --allow-dirty only while iterating")
    root = browser_smoke.create_root(args.root)
    for child in ["logs", "profiles", "proof"]:
        (root / child).mkdir(parents=True, exist_ok=True)
    server, server_thread, base_url = browser_smoke.start_fixture_server(root)
    config = browser_smoke.write_config(args, root)
    parsed = json.loads(config.read_text(encoding="utf-8"))
    parsed.setdefault("browser", {})["video"] = {
        "quality": 40,
        "compression_level": 0,
        "fps": 5,
        "max_width": 320,
        "max_file_bytes": 32 * 1024 * 1024,
    }
    config.write_text(json.dumps(parsed, indent=2) + "\n", encoding="utf-8")
    environment = browser_smoke.smoke_environment(root)
    horizon_log = root / "logs" / f"{args.backend}-video-horizon.log"
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
                root / "logs" / f"{args.backend}-video-mcp.log",
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
    (root / "video-result.json").write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(json.dumps(report, ensure_ascii=False, sort_keys=True), flush=True)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
