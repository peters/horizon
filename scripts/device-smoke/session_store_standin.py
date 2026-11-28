#!/usr/bin/env python3
"""A stand-in for the session stores of the agent programs.

The publisher of the session mirror (`horizon --mirror-sessions`) reads the session
stores of the agent programs of a computer. This program writes such a home from
nothing, with made-up sessions in the shapes that the real programs write, so a test
or a picture of the mirror needs no session of a person.

The home holds two Claude Code sessions, one Codex session and one Pi session. The
first Claude session is *live*: this program starts one process that does nothing and
keeps it alive while it serves, and it writes the registry entry of that process, so
the publisher reports the session as one that a process runs now. The program ends
that process at its own end, and it removes the registry entry, so the stores of the
home never claim a process that is gone.

Usage:

    python3 scripts/device-smoke/session_store_standin.py --home <dir> --serve
    python3 scripts/device-smoke/session_store_standin.py --home <dir> --append "<text>"
    python3 scripts/device-smoke/session_store_standin.py --home <dir> --check

`--serve` writes the home, starts the live process, prints what it wrote, and serves
until SIGINT or SIGTERM. `--append` adds one text of the agent to the live session,
which is the change that a picture of a new entry needs. `--check` reads the home
again and prints the sessions that it holds.

Put the home on `HOME` for the publisher, as a shell does:

    HOME=<dir> horizon --mirror-sessions
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import signal
import sqlite3
import subprocess
import sys
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path

# The made-up sessions of the home. Every id is a UUID of the canonical form, which
# is the form that the mirror accepts.
LIVE_CLAUDE = "11111111-1111-4111-8111-111111111111"
QUIET_CLAUDE = "22222222-2222-4222-8222-222222222222"
CODEX = "33333333-3333-4333-8333-333333333333"
PI = "44444444-4444-4444-8444-444444444444"

# The folder that the sessions report, so a row of the mirror shows one name.
CWD = "/home/agent/demo"
PI_FOLDER = "--home-agent-demo--"
# The time of every line of a store: some minutes before this run, because a session
# older than 14 days is no row of the mirror. The words of a row then read as a time
# of today.
_AT = datetime.now(timezone.utc) - timedelta(minutes=5)
WHEN = _AT.strftime("%Y-%m-%dT%H:%M:%S.000Z")
WHEN_MILLIS = int(_AT.timestamp() * 1000)
WHEN_SECONDS = WHEN_MILLIS // 1000
YEAR = _AT.strftime("%Y")
MONTH = _AT.strftime("%m")
DAY = _AT.strftime("%d")

LIVE_TEXTS = [
    ("person", "Add the retry to the upload of a file."),
    ("agent", "I read the upload path and started the change."),
    ("tool", "Read", "src/upload.rs"),
    ("agent", "The retry is in the upload now."),
]


def write_jsonl(path: Path, lines: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as file:
        for line in lines:
            file.write(json.dumps(line, ensure_ascii=False) + "\n")


def claude_lines(session_id: str, texts: list[tuple[str, ...]]) -> list[dict]:
    lines: list[dict] = []
    for index, text in enumerate(texts):
        if text[0] == "person":
            lines.append(
                {
                    "type": "user",
                    "cwd": CWD,
                    "sessionId": session_id,
                    "isSidechain": False,
                    "timestamp": WHEN,
                    "message": {"role": "user", "content": text[1]},
                }
            )
        elif text[0] == "agent":
            lines.append(
                {
                    "type": "assistant",
                    "cwd": CWD,
                    "sessionId": session_id,
                    "isSidechain": False,
                    "timestamp": WHEN,
                    "message": {
                        "id": f"msg-{index}",
                        "role": "assistant",
                        "content": [{"type": "text", "text": text[1]}],
                    },
                }
            )
        elif text[0] == "tool":
            lines.append(
                {
                    "type": "assistant",
                    "cwd": CWD,
                    "sessionId": session_id,
                    "isSidechain": False,
                    "timestamp": WHEN,
                    "message": {
                        "id": f"msg-{index}",
                        "role": "assistant",
                        "content": [
                            {
                                "type": "tool_use",
                                "id": f"tool-{index}",
                                "name": text[1],
                                "input": {"file_path": text[2]},
                            }
                        ],
                    },
                }
            )
            # The result of the tool is no entry of the mirror, so it is written to
            # show that the publisher leaves it out.
            lines.append(
                {
                    "type": "user",
                    "cwd": CWD,
                    "sessionId": session_id,
                    "timestamp": WHEN,
                    "message": {
                        "role": "user",
                        "content": [
                            {
                                "type": "tool_result",
                                "tool_use_id": f"tool-{index}",
                                "content": "made-up output of the tool",
                            }
                        ],
                    },
                }
            )
    return lines


def last_prompt(session_id: str, text: str) -> dict:
    """The line of a transcript that names the title of its session."""
    return {"type": "last-prompt", "sessionId": session_id, "lastPrompt": text}


def write_codex(home: Path) -> None:
    rollout = home / ".codex" / "sessions" / YEAR / MONTH / DAY
    path = rollout / f"rollout-{YEAR}-{MONTH}-{DAY}T09-40-00-{CODEX}.jsonl"
    write_jsonl(
        path,
        [
            {
                "timestamp": WHEN,
                "type": "session_meta",
                "payload": {"id": CODEX, "cwd": CWD, "timestamp": WHEN},
            },
            {
                "timestamp": WHEN,
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "Name the two new columns."}],
                },
            },
            {
                "timestamp": WHEN,
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "The columns are ready in the plan."}],
                },
            },
            {
                "timestamp": WHEN,
                "type": "response_item",
                "payload": {"type": "function_call", "name": "shell", "arguments": "{\"command\":\"cargo test\"}"},
            },
        ],
    )
    # The list of the Codex sessions is the store that the session pickup reads: one
    # table of threads, whose `rollout_path` names the file of each session.
    store = home / ".codex" / "state_5.sqlite"
    store.parent.mkdir(parents=True, exist_ok=True)
    if store.exists():
        store.unlink()
    connection = sqlite3.connect(store)
    connection.execute(
        "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL, source TEXT NOT NULL,"
        " title TEXT NOT NULL, cwd TEXT NOT NULL, updated_at INTEGER NOT NULL, archived INTEGER NOT NULL DEFAULT 0)"
    )
    connection.execute(
        "INSERT INTO threads VALUES (?, ?, 'cli', 'Name the two new columns.', ?, ?, 0)",
        (CODEX, str(path), CWD, WHEN_SECONDS),
    )
    connection.commit()
    connection.close()


def write_pi(home: Path) -> None:
    path = home / ".pi" / "agent" / "sessions" / PI_FOLDER / f"{YEAR}-{MONTH}-{DAY}T09-40-00-000Z_{PI}.jsonl"
    write_jsonl(
        path,
        [
            {"type": "session", "id": PI, "cwd": CWD, "timestamp": WHEN, "version": "1"},
            {
                "type": "message",
                "id": "entry-1",
                "timestamp": WHEN,
                "message": {"role": "user", "content": [{"type": "text", "text": "Check the flaky test."}]},
            },
            {
                "type": "message",
                "id": "entry-2",
                "timestamp": WHEN,
                "message": {
                    "role": "assistant",
                    "content": [{"type": "text", "text": "The test waits for a file that it never reads."}],
                },
            },
        ],
    )


def write_claude(home: Path) -> None:
    folder = home / ".claude" / "projects" / "-home-agent-demo"
    live = claude_lines(LIVE_CLAUDE, LIVE_TEXTS)
    live.append(last_prompt(LIVE_CLAUDE, "Add the retry to the upload of a file."))
    write_jsonl(folder / f"{LIVE_CLAUDE}.jsonl", live)
    # The second session is its own session, so its lines name its id.
    quiet = claude_lines(
        QUIET_CLAUDE,
        [
            ("person", "Write the release note of the cache."),
            ("agent", "The note is in the draft of the release."),
        ],
    )
    quiet.append(last_prompt(QUIET_CLAUDE, "Write the release note of the cache."))
    write_jsonl(folder / f"{QUIET_CLAUDE}.jsonl", quiet)


def write_home(home: Path) -> None:
    write_claude(home)
    write_codex(home)
    write_pi(home)


def append_agent_text(home: Path, text: str) -> None:
    path = home / ".claude" / "projects" / "-home-agent-demo" / f"{LIVE_CLAUDE}.jsonl"
    lines = [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line]
    index = len(lines)
    lines.insert(
        len(lines),
        {
            "type": "assistant",
            "cwd": CWD,
            "sessionId": LIVE_CLAUDE,
            "timestamp": WHEN,
            "message": {
                "id": f"msg-{index}",
                "role": "assistant",
                "content": [{"type": "text", "text": text}],
            },
        },
    )
    write_jsonl(path, lines)


def live_process(home: Path) -> subprocess.Popen:
    """One process that does nothing, and the registry entry of the live session."""
    child = subprocess.Popen(
        [sys.executable, "-c", "import time; time.sleep(10 ** 6)"],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    registry = home / ".claude" / "sessions"
    registry.mkdir(parents=True, exist_ok=True)
    (registry / f"{child.pid}.json").write_text(
        json.dumps({"pid": child.pid, "sessionId": LIVE_CLAUDE, "cwd": CWD, "status": "idle"}),
        encoding="utf-8",
    )
    return child


def end_live_process(home: Path, child: subprocess.Popen) -> None:
    """Ends the process and removes its registry entry, so no store claims it."""
    entry = home / ".claude" / "sessions" / f"{child.pid}.json"
    entry.unlink(missing_ok=True)
    if child.poll() is None:
        child.terminate()
        try:
            child.wait(timeout=10)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait(timeout=10)


def check(home: Path) -> int:
    problems = []
    for path in [
        home / ".claude" / "projects" / "-home-agent-demo" / f"{LIVE_CLAUDE}.jsonl",
        home / ".claude" / "projects" / "-home-agent-demo" / f"{QUIET_CLAUDE}.jsonl",
        home / ".codex" / "state_5.sqlite",
        home / ".pi" / "agent" / "sessions" / PI_FOLDER / f"{YEAR}-{MONTH}-{DAY}T09-40-00-000Z_{PI}.jsonl",
    ]:
        if not path.exists():
            problems.append(f"missing {path}")
    if problems:
        for problem in problems:
            print(f"session-store-standin: {problem}", file=sys.stderr)
        return 1
    print(f"session-store-standin: Claude {LIVE_CLAUDE} (live) and {QUIET_CLAUDE}, Codex {CODEX}, Pi {PI}")
    return 0


def serve(home: Path) -> int:
    write_home(home)
    child = live_process(home)
    stop = {"asked": False}

    def asked(_signum, _frame):
        stop["asked"] = True

    signal.signal(signal.SIGINT, asked)
    signal.signal(signal.SIGTERM, asked)
    print(f"session-store-standin: home {home}", flush=True)
    print(f"session-store-standin: live process {child.pid} runs Claude {LIVE_CLAUDE}", flush=True)
    print(f"session-store-standin: quiet Claude {QUIET_CLAUDE}, Codex {CODEX}, Pi {PI}", flush=True)
    try:
        while not stop["asked"] and child.poll() is None:
            time.sleep(0.2)
    finally:
        end_live_process(home, child)
        print("session-store-standin: stopped", flush=True)
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="Write a made-up home of agent session stores.")
    parser.add_argument("--home", required=True, type=Path, help="the home that holds the stores")
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--serve", action="store_true", help="write the home and hold the live process")
    mode.add_argument("--append", metavar="TEXT", help="add one text of the agent to the live session")
    mode.add_argument("--check", action="store_true", help="read the home and report its sessions")
    mode.add_argument("--clean", action="store_true", help="remove the home of the stand-in")
    args = parser.parse_args()

    home: Path = args.home
    if args.clean:
        shutil.rmtree(home, ignore_errors=True)
        return 0
    if args.append is not None:
        append_agent_text(home, args.append)
        print("session-store-standin: one text of the agent is in the live session")
        return 0
    if args.check:
        return check(home)
    home.mkdir(parents=True, exist_ok=True)
    return serve(home)


if __name__ == "__main__":
    sys.exit(main())
