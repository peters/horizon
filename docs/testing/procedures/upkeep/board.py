#!/usr/bin/env python3
"""Writes a synthetic Upkeep status document and a Horizon config that shows it.

Usage: board.py <folder>

The folder gets status.json (made-up repositories, times relative to now) and
board.yaml, a config with one workspace that holds an Upkeep board reading that
status. Start Horizon with `--config <folder>/board.yaml --ephemeral`.
"""
import datetime as dt
import json
import sys
from pathlib import Path


def iso(at):
    return at.strftime("%Y-%m-%dT%H:%M:%SZ")


def pull_request(repository, number, title, state, checks, note, updated_at):
    return {
        "repository": repository,
        "number": number,
        "title": title,
        "url": f"https://github.com/{repository}/pull/{number}",
        "author": "dependabot",
        "checks": checks,
        "state": state,
        "note": note,
        "updated_at": iso(updated_at),
    }


def status(now):
    hour = now.replace(minute=0, second=0, microsecond=0)
    runs = []
    for back in range(23, 0, -1):
        start = hour - dt.timedelta(hours=back)
        if back == 5:
            runs.append({"started_at": iso(start), "outcome": "skipped"})
            continue
        minutes = [6, 4, 9, 22, 5, 7, 31, 3][back % 8]
        runs.append(
            {
                "started_at": iso(start),
                "finished_at": iso(start + dt.timedelta(minutes=minutes)),
                "outcome": "failed" if back == 9 else "done",
                "merged": [0, 1, 0, 2, 0, 1, 0, 0][back % 8],
            }
        )
    runs.append({"started_at": iso(hour), "finished_at": iso(hour + dt.timedelta(minutes=3)), "outcome": "done", "merged": 1})
    pulls = [
        # A public Dependabot pull request, so opening a row shows a real page.
        pull_request("peters/horizon", 1516, "chore(deps): bump comrak from 0.55.0 to 0.56.0", "ready", "passed", "Merges in the next run", hour),
        pull_request("acme/web", 409, "Bump the eslint group with 3 updates", "fixing", "failed", "A lint rule changed; fixing on the branch", hour),
        pull_request("acme/api", 88, "Bump serde from 1.0.226 to 1.0.228", "waiting_checks", "running", "", hour),
        pull_request("acme/api", 86, "Bump tokio from 1.47.1 to 1.48.0", "blocked", "passed", "Needs a deployed preview that CI skips for bots", hour - dt.timedelta(hours=2)),
        pull_request("acme/mobile", 31, "Bump the androidx group with 2 updates", "needs_review", "passed", "The repository requires a review from a person", hour - dt.timedelta(hours=5)),
        pull_request("acme/docs", 17, "Bump actions/checkout from 5 to 6", "merged", "passed", "", hour),
        pull_request("acme/web", 401, "Bump typescript from 5.9.2 to 5.9.3", "merged", "passed", "", hour - dt.timedelta(days=1)),
        pull_request("acme/api", 80, "Bump rustls from 0.23.31 to 0.23.32", "merged", "passed", "", hour - dt.timedelta(days=9)),
    ]
    activity = [
        (0, "Run started"),
        (1, "Read 5 repositories with 6 open Dependabot pull requests before the run"),
        (1, "acme/docs#17: every gate passed; squash-merged"),
        (2, "acme/web#409: lint failed after the update; fixing on the branch"),
        (3, "Run finished: 1 merged, 1 fixing, 1 waiting on checks"),
    ]
    return {
        "version": 1,
        "agent": "codex",
        "worker": {"name": "Hetzner cx23", "location": "Helsinki", "state": "connected"},
        "schedule": {"every_minutes": 60, "next_run_at": iso(hour + dt.timedelta(hours=1)), "paused": False},
        "repositories": ["acme/api", "acme/docs", "acme/mobile", "acme/web", "peters/horizon"],
        "runs": runs,
        "pull_requests": pulls,
        "activity": [{"at": iso(hour + dt.timedelta(minutes=m)), "text": t} for m, t in activity],
    }


def config(path):
    return f"""version: 11
window:
  width: 1600.0
  height: 1000.0
workspaces:
- name: Upkeep
  position: [0.0, 40.0]
  terminals:
  - name: Upkeep
    kind: upkeep
    command: {json.dumps(str(path))}
    position: [40.0, 80.0]
    size: [860.0, 820.0]
"""


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    folder = Path(sys.argv[1]).resolve()
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / "status.json"
    path.write_text(json.dumps(status(dt.datetime.now(dt.timezone.utc)), indent=1))
    (folder / "board.yaml").write_text(config(path))
    print(folder / "board.yaml")


if __name__ == "__main__":
    main()
