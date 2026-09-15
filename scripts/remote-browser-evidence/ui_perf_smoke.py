#!/usr/bin/env python3
"""UI smoke and performance comparison for remote browser panels (peters/horizon#628).

Horizon runs headless on a private X display with one remote target at the
hosted grid and, for comparison, the local Chromium backend. Each panel goes
through four phases of equal length, driven only through the public MCP tools
and the window manager:

  idle         nothing happens on the page or the panel
  interaction  a `browser_act scroll` every second (frames follow each act)
  hidden       `browser_visibility visible=false` for the whole phase
  resizing     the Horizon window is resized every second with xdotool

During every phase the Horizon process is sampled once a second from
`/proc`: CPU time of the whole process, of the main (UI) thread and of the
`browser-driver` thread that owns the WebDriver connection, plus resident
memory. For the remote panel the provider's own command log is fetched after
the session is released and its screenshot requests are bucketed per phase,
which is the external count of Horizon's adaptive screenshot polling.

Credentials come from the mode-600 netrc and go only into the Secret Service
under Horizon's items, restored afterwards, exactly as `live_smoke.py` does.
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import pathlib
import re
import signal
import subprocess
import sys
import time

HERE = pathlib.Path(__file__).resolve().parent
REPO = pathlib.Path(os.environ["HORIZON_REPO"]) if os.environ.get("HORIZON_REPO") else HERE.parents[1]
sys.path.insert(0, str(REPO / "scripts" / "remote-browser-evidence"))
sys.path.insert(0, str(REPO / "scripts" / "browser-smoke"))
from live_smoke import (  # noqa: E402
    API,
    FIXTURE,
    OPENER,
    RPC_LOG,
    TARGETS,
    api_get,
    handshake,
    load_credential,
    provider_session_state,
    raw,
    restore_keyring,
    seed_keyring,
    wait_for,
    write_config,
)
from mcp_gate import McpClient  # noqa: E402

CLK_TCK = os.sysconf("SC_CLK_TCK")
PAGE = os.sysconf("SC_PAGE_SIZE")
PHASES = ("idle", "interaction", "hidden", "resizing")
SIZES = ((1400, 900), (1100, 700))


def utc_now() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def value_of(result: dict, key: str = "value"):
    return (result.get("structuredContent") or {}).get(key)


# --- /proc sampling -----------------------------------------------------------

def stat_ticks(path: pathlib.Path) -> tuple[str, int]:
    text = path.read_text(encoding="utf-8", errors="replace")
    comm = text[text.index("(") + 1 : text.rindex(")")]
    fields = text[text.rindex(")") + 2 :].split()
    return comm, int(fields[11]) + int(fields[12])


def snapshot(pid: int) -> dict:
    """CPU ticks for the process, the main thread and the driver thread(s),
    and resident memory in bytes."""
    proc = pathlib.Path(f"/proc/{pid}")
    _, total = stat_ticks(proc / "stat")
    main = driver = 0
    for task in (proc / "task").iterdir():
        try:
            comm, ticks = stat_ticks(task / "stat")
        except (FileNotFoundError, ProcessLookupError, ValueError):
            continue
        if task.name == str(pid):
            main = ticks
        elif comm == "browser-driver":
            driver += ticks
    rss = int((proc / "statm").read_text().split()[1]) * PAGE
    return {"total": total, "main": main, "driver": driver, "rss": rss, "at": time.time()}


def summarise(samples: list[dict]) -> dict:
    first, last = samples[0], samples[-1]
    seconds = max(last["at"] - first["at"], 1e-6)
    pct = lambda key: round((last[key] - first[key]) / CLK_TCK / seconds * 100, 1)  # noqa: E731
    return {
        "seconds": round(seconds, 1),
        "cpu_percent_total": pct("total"),
        "cpu_percent_main_thread": pct("main"),
        "cpu_percent_driver_thread": pct("driver"),
        "rss_mb_start": round(first["rss"] / 1e6, 1),
        "rss_mb_end": round(last["rss"] / 1e6, 1),
    }


# --- phases -------------------------------------------------------------------

def window_id(display: str) -> str | None:
    out = subprocess.run(["xdotool", "search", "--onlyvisible", "--name", "Horizon"], capture_output=True, text=True,
                         env={**os.environ, "DISPLAY": display}, check=False).stdout.split()
    return out[0] if out else None


def resize(wid: str | None, display: str, width: int, height: int) -> bool:
    """One window resize through the window manager; a missing window or a
    failed `xdotool` call is a phase failure, not a silent skip."""
    if not wid:
        return False
    done = subprocess.run(["xdotool", "windowsize", wid, str(width), str(height)], env={**os.environ, "DISPLAY": display}, check=False)
    return done.returncode == 0


def run_phase(name: str, seconds: float, pid: int, client: McpClient, panel_id: str, display: str, wid: str | None) -> dict:
    """One phase: ticks are scheduled against an absolute deadline (one per
    second from the phase start), so every subject gets the same number of
    actions and samples whatever each action's latency; the counts are
    recorded so the comparison can be checked."""
    samples = [snapshot(pid)]
    started = time.monotonic()
    wall_started = time.time()
    tick = 0
    actions = 0
    errors = 0
    if name == "hidden":
        hidden = raw(client, "browser_visibility", {"panel_id": panel_id, "visible": False})
        errors += bool(hidden.get("isError"))
    while True:
        tick += 1
        due = started + tick
        if due > started + seconds:
            break
        time.sleep(max(0.0, due - time.monotonic()))
        if name == "interaction":
            acted = raw(client, "browser_act", {"panel_id": panel_id, "action": "scroll", "delta_y": 300 if tick % 2 else -300})
            actions += 1
            errors += bool(acted.get("isError"))
        elif name == "resizing":
            width, height = SIZES[tick % 2]
            actions += 1
            errors += not resize(wid, display, width, height)
        samples.append(snapshot(pid))
    if name == "hidden":
        shown = raw(client, "browser_visibility", {"panel_id": panel_id, "visible": True})
        errors += bool(shown.get("isError"))
    if name == "resizing":
        errors += not resize(wid, display, 1400, 900)
    ended = time.time()
    return {"phase": name, "started": wall_started, "ended": ended, "actions": actions, "actions_failed": errors,
            "samples": len(samples), **summarise(samples)}


def api_text(url: str, auth: str) -> str:
    """The provider's raw command log for one session (text, not JSON)."""
    import urllib.request

    if not url.startswith(API + "/"):
        raise SystemExit(f"refusing to send the credential to {url}")
    request = urllib.request.Request(url, headers={"Authorization": auth, "Accept": "text/plain"})
    with OPENER.open(request, timeout=60) as response:
        return response.read().decode("utf-8", errors="replace")


LOG_STAMP = re.compile(r"^(\d{4})-(\d{1,2})-(\d{1,2}) (\d{1,2}):(\d{1,2}):(\d{1,2}):(\d{1,3}) REQUEST .* (GET|POST|DELETE) (\S+)")


def screenshot_requests_per_phase(auth: str, session_name: str, phases: list[dict]) -> dict:
    """Bucket the provider's logged WebDriver commands by phase (its log
    timestamps are UTC)."""
    import calendar

    builds = api_get(f"{API}/automate/builds.json?limit=5", auth)
    hashed = None
    for item in builds.get("body") or []:
        build = item.get("automation_build", {})
        sessions = api_get(f"{API}/automate/builds/{build.get('hashed_id')}/sessions.json", auth)
        for entry in sessions.get("body") or []:
            session = entry.get("automation_session", {})
            if session.get("name") == session_name:
                hashed = session.get("hashed_id")
                break
        if hashed:
            break
    if not hashed:
        return {"note": "session not found"}
    text = api_text(f"{API}/automate/sessions/{hashed}/logs", auth)
    counts = {phase["phase"]: {"screenshot": 0, "other": 0} for phase in phases}
    total = {"screenshot": 0, "other": 0}
    for line in text.splitlines():
        match = LOG_STAMP.match(line)
        if not match:
            continue
        y, mo, d, h, mi, s, ms = (int(part) for part in match.groups()[:7])
        at = calendar.timegm((y, mo, d, h, mi, s)) + ms / 1000
        kind = "screenshot" if match.group(9).endswith("/screenshot") else "other"
        total[kind] += 1
        for phase in phases:
            if phase["started"] <= at <= phase["ended"]:
                counts[phase["phase"]][kind] += 1
                break
    for phase in phases:
        bucket = counts[phase["phase"]]
        bucket["screenshot_per_second"] = round(bucket["screenshot"] / max(phase["seconds"], 1e-6), 2)
    return {"session": hashed, "per_phase": counts, "total": total}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--horizon", required=True)
    parser.add_argument("--display", default=":99")
    # Only hosted-grid targets: the release proof and the command log below
    # are that provider's REST API.
    hosted = sorted(name for name, target in TARGETS.items() if target["provider"] == "browserstack")
    parser.add_argument("--target", default="ios_phone", choices=hosted)
    parser.add_argument("--local-backend", default="chromium")
    parser.add_argument("--phase-seconds", type=float, default=20.0)
    parser.add_argument("--subjects", nargs="+", default=["remote", "local"], choices=["remote", "local"])
    # Not under a hidden directory: the snap-packaged Chromium used for the
    # local comparison cannot open a profile there.
    parser.add_argument("--out", default=str(pathlib.Path("~/horizon-628-perf").expanduser()))
    args = parser.parse_args()
    root = pathlib.Path(args.out) / f"run-{int(time.time())}"
    (root / "home").mkdir(parents=True)
    session_names = {name: f"ui-perf-{name}-{root.name}" for name in TARGETS}
    login = password = auth = None
    if "remote" in args.subjects:
        login, password = load_credential()
        auth = "Basic " + base64.b64encode(f"{login}:{password}".encode()).decode()
    os.environ["HOME"] = str(root / "home")
    os.environ["DISPLAY"] = args.display
    os.environ["RUST_LOG"] = "info"
    for name in ("HORIZON", "HORIZON_BROWSER_ACTOR", "HORIZON_BROWSER_HOST_INSTANCE"):
        os.environ.pop(name, None)
    env = dict(os.environ)
    config = write_config(root, [args.target] if "remote" in args.subjects else [], session_names)
    log = (root / "horizon.log").open("w", encoding="utf-8")
    report: dict = {"started": utc_now(), "phase_seconds": args.phase_seconds, "subjects": {}}
    # A termination signal must unwind through the finally below so the
    # keyring is restored and Horizon is stopped; Python's default would
    # exit at once.
    restored_handlers = {}
    for name in ("SIGTERM", "SIGHUP"):
        sig = getattr(signal, name, None)
        if sig is not None:
            restored_handlers[sig] = signal.signal(sig, lambda signum, frame: (_ for _ in ()).throw(SystemExit(128 + signum)))
    previous = seed_keyring(login, password) if login is not None else {}
    app = None
    try:
        app = subprocess.Popen([args.horizon, "--config", str(config), "--ephemeral"], env=env, stdout=log, stderr=log)
        actor = wait_for(root / "agent-actor", 90)
        host_instance = wait_for(root / "agent-host-instance", 30)
        time.sleep(3)
        wid = window_id(args.display)
        report["window_found"] = wid is not None
        baseline = [snapshot(app.pid)]
        time.sleep(5)
        baseline.append(snapshot(app.pid))
        report["baseline_no_panel"] = summarise(baseline)
        subjects = [("remote", {"target": args.target}), ("local", {"backend": args.local_backend})]
        for label, create_args in subjects:
            if label not in args.subjects:
                continue
            client = McpClient(pathlib.Path(args.horizon), root / f"mcp-{label}.log", 200.0, actor, host_instance)
            RPC_LOG[:] = [root / f"rpc-{label}.jsonl"]
            entry: dict = {"create": create_args, "phases": []}
            try:
                handshake(client)
                started = time.monotonic()
                created = raw(client, "browser_create", {**create_args, "url": FIXTURE, "timeout_millis": 90000})
                entry["browser_create"] = {"ms": int((time.monotonic() - started) * 1000), "is_error": created.get("isError"),
                                           "navigation": value_of(created, "navigation")}
                panel = value_of(created, "panel") or {}
                panel_id = panel.get("panel_id")
                if panel_id:
                    entry["panel"] = {k: panel.get(k) for k in ("backend", "remote_target", "remote_device", "protocol")}
                    time.sleep(3)
                    for phase in PHASES:
                        entry["phases"].append(run_phase(phase, args.phase_seconds, app.pid, client, panel_id, args.display, wid))
                        (root / "report.json").write_text(json.dumps(report | {"subjects": report["subjects"] | {label: entry}}, indent=2) + "\n")
                    closed = raw(client, "browser_close", {"panel_id": panel_id, "timeout_millis": 60000})
                    entry["browser_close"] = {"is_error": closed.get("isError"), "closed": value_of(closed, "closed")}
            finally:
                client.close()
            if label == "remote" and entry.get("phases"):
                time.sleep(5)
                entry["provider_release_proof"] = provider_session_state(auth, session_names[args.target])
                entry["provider_command_log"] = screenshot_requests_per_phase(auth, session_names[args.target], entry["phases"])
            report["subjects"][label] = entry
            (root / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    finally:
        if app is not None:
            app.terminate()
            try:
                app.wait(timeout=20)
            except subprocess.TimeoutExpired:
                app.kill()
        log.close()
        if previous:
            restore_keyring(previous)
        for sig, handler in restored_handlers.items():
            signal.signal(sig, handler)
    problems = []
    for label, entry in report["subjects"].items():
        if entry.get("browser_create", {}).get("is_error") or not entry.get("phases"):
            problems.append(f"{label}: panel not created")
            continue
        if any(phase["actions_failed"] for phase in entry["phases"]):
            problems.append(f"{label}: an action failed during a phase")
        if entry.get("browser_close", {}).get("closed") is not True:
            problems.append(f"{label}: not closed")
        if label == "remote" and not entry.get("provider_release_proof", {}).get("terminal"):
            problems.append("remote: provider did not report the session terminal")
        if label == "remote":
            # The external screenshot count is the point of the run: the
            # session must be found and the interaction phase must show
            # captures, or nothing was measured.
            command_log = entry.get("provider_command_log") or {}
            interaction = (command_log.get("per_phase") or {}).get("interaction") or {}
            if not command_log.get("session") or not interaction.get("screenshot"):
                problems.append("remote: provider command log missing or without interaction screenshots")
    counts = {label: [phase["actions"] for phase in entry.get("phases", [])] for label, entry in report["subjects"].items()}
    if len({tuple(c) for c in counts.values()}) > 1:
        problems.append(f"subjects ran different action counts: {counts}")
    report["failures"] = problems
    report["passed"] = not problems
    (root / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
