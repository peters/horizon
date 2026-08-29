#!/usr/bin/env python3
"""Launch an isolated Horizon browser smoke workspace on Linux or macOS."""

from __future__ import annotations

import argparse
import json
import os
import platform
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path
from typing import Any, Sequence

# Import the committed fixture helper without dirtying the exact-head checkout.
sys.dont_write_bytecode = True
import fixture_server


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--backend", choices=["chromium", "firefox", "safari"], required=True)
    parser.add_argument("--horizon", type=Path, default=Path("target/debug/horizon"))
    parser.add_argument("--root", type=Path, help="reuse a prior isolated root for persistence checks")
    parser.add_argument("--chromium-command", type=Path)
    parser.add_argument("--firefox-command", type=Path)
    parser.add_argument("--geckodriver-command", type=Path)
    parser.add_argument("--safaridriver-command", type=Path)
    parser.add_argument(
        "--automation-disclosure",
        choices=["minimize_common_signals", "browser_default"],
        default="minimize_common_signals",
    )
    parser.add_argument("--skip-mcp", action="store_true", help="launch only the visual/manual lane")
    parser.add_argument("--skip-handoff", action="store_true", help="omit the visible handoff/hand-back step")
    parser.add_argument("--ephemeral", action="store_true", help="disable persistence for a focused non-restore run")
    parser.add_argument(
        "--allow-dirty",
        action="store_true",
        help="allow an iterative run from a dirty checkout; final gate evidence must omit this flag",
    )
    return parser.parse_args(argv)


def resolve_executable(path: Path) -> Path:
    resolved = path.expanduser().resolve()
    if not resolved.is_file() or not os.access(resolved, os.X_OK):
        raise SystemExit(f"executable does not exist or is not runnable: {resolved}")
    return resolved


def optional_executable(path: Path | None) -> str | None:
    if path is None:
        return None
    expanded = path.expanduser()
    absolute = expanded if expanded.is_absolute() else Path.cwd() / expanded
    if not absolute.is_file() or not os.access(absolute, os.X_OK):
        raise SystemExit(f"executable does not exist or is not runnable: {absolute}")
    # Keep identity-bearing launcher symlinks intact. In particular,
    # /snap/bin/chromium points at the generic snap executable, which selects
    # the Chromium app from argv[0]; canonicalizing it changes the program.
    return str(absolute.absolute())


def validate_platform(backend: str) -> None:
    system = platform.system()
    if system not in {"Linux", "Darwin"}:
        raise SystemExit(f"the reusable browser smoke runner supports Linux and macOS, not {system}")
    if backend == "safari" and system != "Darwin":
        raise SystemExit("Safari smoke requires macOS")
    if system == "Linux" and not os.environ.get("DISPLAY") and not os.environ.get("WAYLAND_DISPLAY"):
        raise SystemExit("Linux smoke requires a GUI display; prefer a task-owned Xvfb plus window manager")


def create_root(requested: Path | None) -> Path:
    if requested is None:
        return Path(tempfile.mkdtemp(prefix="horizon-browser-smoke."))
    root = requested.expanduser().resolve()
    root.mkdir(parents=True, exist_ok=True)
    return root


def start_fixture_server(root: Path) -> tuple[Any, threading.Thread, str]:
    port_file = root / "fixture-port"
    port = int(port_file.read_text(encoding="utf-8").strip()) if port_file.exists() else 0
    try:
        server = fixture_server.create_server(port)
    except OSError as error:
        raise SystemExit(f"could not reuse fixture port {port}: {error}") from error
    if port == 0:
        port_file.write_text(f"{server.server_port}\n", encoding="utf-8")
    thread = threading.Thread(target=server.serve_forever, name="browser-smoke-fixture", daemon=True)
    thread.start()
    return server, thread, f"http://127.0.0.1:{server.server_port}"


def browser_config(args: argparse.Namespace, root: Path) -> dict[str, Any]:
    config: dict[str, Any] = {
        "backend": args.backend,
        "automation_disclosure": args.automation_disclosure,
        "profile_root": str(root / "profiles" / args.backend),
        "quality": 70,
        "every_nth_frame": 1,
    }
    optional = {
        "command": optional_executable(args.chromium_command),
        "firefox_command": optional_executable(args.firefox_command),
        "geckodriver_command": optional_executable(args.geckodriver_command),
        "safaridriver_command": optional_executable(args.safaridriver_command),
    }
    config.update({key: value for key, value in optional.items() if value is not None})
    return config


def write_config(args: argparse.Namespace, root: Path) -> Path:
    actor_path = root / "agent-actor"
    actor_probe = (
        "import os,pathlib,time;"
        f"pathlib.Path({str(actor_path)!r}).write_text(os.environ['HORIZON_BROWSER_ACTOR'],encoding='utf-8');"
        "time.sleep(3600)"
    )
    config = {
        "version": 10,
        "window": {"width": 1400, "height": 900},
        "browser": browser_config(args, root),
        "workspaces": [
            {
                "name": f"{args.backend.title()} Browser Smoke",
                "position": [30, 30],
                "terminals": [
                    {
                        "name": "Browser MCP Smoke Agent",
                        "kind": "codex",
                        "command": sys.executable,
                        "args": ["-c", actor_probe],
                        "position": [30, 30],
                        "size": [620, 360],
                    }
                ],
            }
        ],
    }
    config_path = root / f"{args.backend}.json"
    config_path.write_text(json.dumps(config, indent=2) + "\n", encoding="utf-8")
    return config_path


def git_head(repo_root: Path) -> str | None:
    result = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=repo_root,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    return result.stdout.strip() if result.returncode == 0 else None


def git_changes(repo_root: Path) -> list[str]:
    result = subprocess.run(
        ["git", "status", "--porcelain=v1", "--untracked-files=all"],
        cwd=repo_root,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or "unknown git status error"
        raise SystemExit(f"could not establish candidate checkout state: {detail}")
    return result.stdout.splitlines()


def smoke_environment(root: Path) -> dict[str, str]:
    environment = os.environ.copy()
    environment.pop("HORIZON", None)
    environment.pop("HORIZON_BROWSER_ACTOR", None)
    environment["HOME"] = str(root / "home")
    environment["XDG_CACHE_HOME"] = str(root / "cache")
    environment["XDG_CONFIG_HOME"] = str(root / "xdg-config")
    environment["RUST_LOG"] = environment.get("RUST_LOG", "info")
    for path in [environment["HOME"], environment["XDG_CACHE_HOME"], environment["XDG_CONFIG_HOME"]]:
        Path(path).mkdir(parents=True, exist_ok=True)
    return environment


def run_mcp_gate(
    args: argparse.Namespace,
    command: Path,
    root: Path,
    base_url: str,
    environment: dict[str, str],
) -> int:
    actor_path = root / "agent-actor"
    deadline = time.monotonic() + 30
    actor = ""
    while time.monotonic() < deadline:
        try:
            actor = actor_path.read_text(encoding="utf-8").strip()
        except OSError:
            actor = ""
        if actor.startswith("horizon:"):
            break
        time.sleep(0.1)
    else:
        print("agent panel did not publish its Horizon browser actor", file=sys.stderr, flush=True)
        return 1
    script = Path(__file__).resolve().parent / "mcp_gate.py"
    invocation = [
        sys.executable,
        str(script),
        "--horizon",
        str(command),
        "--backend",
        args.backend,
        "--base-url",
        base_url,
        "--automation-disclosure",
        args.automation_disclosure,
        "--log",
        str(root / "logs" / f"{args.backend}-mcp.log"),
        "--actor",
        actor,
    ]
    if not args.skip_handoff:
        invocation.append("--handoff")
    return subprocess.run(invocation, env=environment, check=False).returncode


def process_table() -> dict[int, tuple[int, str]] | None:
    result = subprocess.run(
        ["ps", "-axo", "pid=,ppid=,command="],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    if result.returncode != 0:
        return None
    table: dict[int, tuple[int, str]] = {}
    for line in result.stdout.splitlines():
        fields = line.strip().split(maxsplit=2)
        if len(fields) < 2:
            continue
        command = fields[2] if len(fields) == 3 else ""
        try:
            table[int(fields[0])] = (int(fields[1]), command)
        except ValueError:
            continue
    return table


class ProcessTracker:
    """Remember exact candidate descendants so reparented survivors stay visible."""

    def __init__(self, root_pid: int) -> None:
        self.root_pid = root_pid
        self.identities: dict[int, set[str]] = {}
        self.sampled = False
        self.stop_requested = threading.Event()
        self.thread = threading.Thread(target=self._run, name="browser-smoke-process-tracker", daemon=True)

    def start(self) -> None:
        self.thread.start()

    def stop(self) -> None:
        self.stop_requested.set()
        self.thread.join(timeout=2)

    def _run(self) -> None:
        while not self.stop_requested.is_set():
            table = process_table()
            if table is not None:
                self.sampled = True
                descendants = {self.root_pid}
                changed = True
                while changed:
                    changed = False
                    for pid, (parent, _) in table.items():
                        if pid not in descendants and parent in descendants:
                            descendants.add(pid)
                            changed = True
                for pid in descendants:
                    if pid == self.root_pid or pid not in table:
                        continue
                    self.identities.setdefault(pid, set()).add(table[pid][1])
            self.stop_requested.wait(0.25)

    def survivors(self) -> list[str]:
        if not self.sampled:
            return ["process tracker could not read the platform process table"]
        table = process_table()
        if table is None:
            return ["process tracker could not read the final platform process table"]
        return [
            f"{pid} {table[pid][1]}"
            for pid, commands in sorted(self.identities.items())
            if pid in table and table[pid][1] in commands
        ]


def root_path_survivors(root: Path) -> list[str]:
    table = process_table()
    if table is None:
        return ["root-path process scan could not read the platform process table"]
    ancestry: set[int] = set()
    current = os.getpid()
    while current > 0 and current not in ancestry:
        ancestry.add(current)
        current = table.get(current, (0, ""))[0]
    root_text = str(root)
    browser_markers = ("chrom", "firefox", "geckodriver", "safari")
    return [
        f"{pid} {command}"
        for pid, (_, command) in sorted(table.items())
        if pid not in ancestry
        and root_text in command
        and any(marker in command.lower() for marker in browser_markers)
    ]


def active_manifests(root: Path) -> list[str]:
    runtime = root / "home" / ".horizon" / "runtime" / "browsers"
    return sorted(str(path) for path in runtime.glob("*.json")) if runtime.is_dir() else []


def audit_journals(root: Path) -> tuple[list[str], list[str]]:
    audit_root = root / "home" / ".horizon" / "audit" / "browsers"
    paths = sorted(audit_root.glob("*.jsonl")) if audit_root.is_dir() else []
    journals = [str(path) for path in paths]
    findings = [
        f"audit journal mode is {path.stat().st_mode & 0o777:o}, expected 600: {path}"
        for path in paths
        if path.stat().st_mode & 0o777 != 0o600
    ]
    return journals, findings


def network_capture_findings(root: Path, backend: str, expect_exports: bool) -> tuple[list[str], list[str]]:
    # Captures live beneath each persistent panel profile so permanent panel
    # and saved-session deletion removes them together. The isolated smoke
    # root may use either Horizon's default profile root or a configured one.
    paths = sorted(root.rglob("*.ndjson"))
    findings: list[str] = []
    if expect_exports and backend != "safari" and len(paths) < 2:
        findings.append("MCP lane did not produce both stopped and active-close network exports")
    if backend == "safari" and paths:
        findings.append("unsupported Safari network capture unexpectedly created an export")
    for path in paths:
        if os.name != "nt" and path.stat().st_mode & 0o777 != 0o600:
            findings.append(f"network capture mode is {path.stat().st_mode & 0o777:o}, expected 600: {path}")
        try:
            records = [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines()]
        except (OSError, json.JSONDecodeError) as error:
            findings.append(f"network capture is not readable NDJSON: {path}: {error}")
            continue
        if not records or records[0].get("kind") != "capture_started":
            findings.append(f"network capture has no start marker: {path}")
        if not records or records[-1].get("kind") != "capture_stopped":
            findings.append(f"network capture did not flush on stop/normal close: {path}")
        sequences = [record.get("sequence") for record in records]
        if any(not isinstance(sequence, int) for sequence in sequences) or sequences != sorted(set(sequences)):
            findings.append(f"network capture sequence is not monotonic and unique: {path}")
    return [str(path) for path in paths], findings


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    validate_platform(args.backend)
    repo_root = Path(__file__).resolve().parents[2]
    command = resolve_executable(args.horizon if args.horizon.is_absolute() else repo_root / args.horizon)
    head = git_head(repo_root)
    if head is None:
        raise SystemExit("could not resolve the candidate git HEAD")
    changes = git_changes(repo_root)
    if changes and not args.allow_dirty:
        preview = "\n".join(changes[:20])
        suffix = "\n..." if len(changes) > 20 else ""
        raise SystemExit(
            "final browser smoke requires a clean exact-head checkout; "
            "commit/stash the changes or use --allow-dirty only for iterative diagnosis:\n"
            f"{preview}{suffix}"
        )
    root = create_root(args.root)
    for child in ["logs", "profiles", "proof"]:
        (root / child).mkdir(parents=True, exist_ok=True)
    server, server_thread, base_url = start_fixture_server(root)
    config_path = write_config(args, root)
    environment = smoke_environment(root)
    started_at = time.time()
    metadata = {
        "backend": args.backend,
        "base_url": base_url,
        "config": str(config_path),
        "git_head": head,
        "git_dirty": bool(changes),
        "horizon": str(command),
        "platform": platform.platform(),
        "root": str(root),
        "started_at": started_at,
    }
    (root / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(metadata, sort_keys=True), flush=True)

    launch = [str(command), "--config", str(config_path)]
    if args.ephemeral:
        launch.append("--ephemeral")
    horizon_log_path = root / "logs" / f"{args.backend}-horizon.log"
    mcp_status = 0
    candidate_status = 1
    tracker: ProcessTracker | None = None
    try:
        with horizon_log_path.open("w", encoding="utf-8") as horizon_log:
            candidate = subprocess.Popen(launch, env=environment, stdout=horizon_log, stderr=subprocess.STDOUT)
            tracker = ProcessTracker(candidate.pid)
            tracker.start()
            print(
                json.dumps(
                    {
                        "candidate_pid": candidate.pid,
                        "instruction": "Use only this PID/window; the harness waits for a normal window close",
                    },
                    sort_keys=True,
                ),
                flush=True,
            )
            if not args.skip_mcp:
                mcp_status = run_mcp_gate(args, command, root, base_url, environment)
                if mcp_status == 0:
                    print(
                        "MCP gate passed. Complete the platform lane in docs/testing/browser-panel-gate.md, "
                        "capture proof under the printed root, then close this exact window normally.",
                        flush=True,
                    )
                else:
                    print(
                        "MCP gate failed. Preserve the logs, inspect the exact smoke window, then close it normally.",
                        file=sys.stderr,
                        flush=True,
                    )
            else:
                print(
                    "Complete the requested visual/persistence lane, capture proof, then close this exact window normally.",
                    flush=True,
                )
            candidate_status = candidate.wait()
    finally:
        if tracker is not None:
            tracker.stop()
        server.shutdown()
        server.server_close()
        server_thread.join(timeout=5)

    manifests = active_manifests(root)
    journals, audit_findings = audit_journals(root)
    if not args.skip_mcp and not journals:
        audit_findings.append("MCP lane produced no persistent audit journal")
    capture_exports, capture_findings = network_capture_findings(root, args.backend, not args.skip_mcp)
    tracked_process_count = len(tracker.identities) if tracker is not None else 0
    survivors = sorted(set((tracker.survivors() if tracker is not None else []) + root_path_survivors(root)))
    report = {
        **metadata,
        "candidate_status": candidate_status,
        "finished_at": time.time(),
        "mcp_status": mcp_status,
        "audit_journals": journals,
        "audit_permission_findings": audit_findings,
        "network_capture_exports": capture_exports,
        "network_capture_findings": capture_findings,
        "remaining_manifests": manifests,
        "surviving_browser_processes": survivors,
        "tracked_process_count": tracked_process_count,
    }
    report_path = root / "report.json"
    report_path.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({**report, "report": str(report_path)}, sort_keys=True), flush=True)
    if candidate_status != 0 or mcp_status != 0 or audit_findings or capture_findings or manifests or survivors:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
